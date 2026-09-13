use std::future::Future;
use std::thread;
use std::time::{Duration, Instant};

use mtp_rs::MtpDevice;

use crate::device::{
    TP7_PRODUCT_ID, TP7_VENDOR_ID, Tp7Device, UsbMode, list_tp7_devices, select_one_device,
};
use crate::midi::{MidiSwitchReport, switch_tp7_to_mtp};
use crate::output::AppError;

pub const KNOWN_TP7: &[(u16, u16)] = &[(TP7_VENDOR_ID, TP7_PRODUCT_ID)];

#[derive(Debug, Clone, Copy)]
pub enum MtpOpenPolicy {
    MtpOnly,
    AutoSwitch,
    RequireAutoConnectFlag,
}

#[derive(Debug, Clone)]
pub struct PreparedMtpDevice {
    pub initial_usb: Tp7Device,
    pub usb: Tp7Device,
    pub switched: bool,
    pub midi_switch: Option<MidiSwitchReport>,
}

pub struct MtpSession {
    pub prepared: PreparedMtpDevice,
    pub device: MtpDevice,
}

impl MtpSession {
    pub async fn close(self) -> Result<(), AppError> {
        self.device.close().await.map_err(map_mtp_error)
    }
}

pub fn block_on<T, F>(future: F) -> Result<T, AppError>
where
    F: Future<Output = Result<T, AppError>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| AppError::Runtime {
            message: error.to_string(),
        })?;

    runtime.block_on(future)
}

pub fn prepare_mtp_device(
    serial: Option<&str>,
    policy: MtpOpenPolicy,
) -> Result<PreparedMtpDevice, AppError> {
    let initial_usb = wait_for_tp7_device(serial, Duration::from_secs(4))?;

    if is_mtp_visible(&initial_usb) {
        return Ok(PreparedMtpDevice {
            initial_usb: initial_usb.clone(),
            usb: initial_usb,
            switched: false,
            midi_switch: None,
        });
    }

    if initial_usb.mode != UsbMode::AudioMidi {
        return Err(AppError::MtpNotVisible {
            serial: serial_for_error(&initial_usb),
            mode: initial_usb.mode.to_string(),
        });
    }

    match policy {
        MtpOpenPolicy::AutoSwitch => {}
        MtpOpenPolicy::MtpOnly => {
            return Err(AppError::MtpNotVisible {
                serial: serial_for_error(&initial_usb),
                mode: initial_usb.mode.to_string(),
            });
        }
        MtpOpenPolicy::RequireAutoConnectFlag => {
            return Err(AppError::AutoConnectRequired {
                serial: serial_for_error(&initial_usb),
                mode: initial_usb.mode.to_string(),
            });
        }
    }

    let midi_switch = switch_tp7_to_mtp_with_retry(&initial_usb, Duration::from_secs(12))?;
    let usb = wait_for_mtp(&initial_usb, serial, Duration::from_secs(12))?;

    Ok(PreparedMtpDevice {
        initial_usb,
        usb,
        switched: true,
        midi_switch: Some(midi_switch),
    })
}

fn wait_for_tp7_device(serial: Option<&str>, timeout: Duration) -> Result<Tp7Device, AppError> {
    let deadline = Instant::now() + timeout;

    loop {
        match select_one_device(list_tp7_devices()?, serial) {
            Ok(device) => return Ok(device),
            Err(error) if is_transient_device_absence(&error) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(250));
            }
            Err(error) => return Err(error),
        }
    }
}

pub async fn open_mtp_session(
    serial: Option<&str>,
    policy: MtpOpenPolicy,
) -> Result<MtpSession, AppError> {
    let prepared = prepare_mtp_device(serial, policy)?;
    open_prepared_mtp_session(prepared).await
}

pub async fn open_prepared_mtp_session(
    prepared: PreparedMtpDevice,
) -> Result<MtpSession, AppError> {
    let device = open_mtp_device(prepared.usb.serial_number.as_deref()).await?;

    Ok(MtpSession { prepared, device })
}

pub async fn open_mtp_device(serial: Option<&str>) -> Result<MtpDevice, AppError> {
    match serial {
        Some(serial) => {
            MtpDevice::builder()
                .known_devices(KNOWN_TP7)
                .open_by_serial(serial)
                .await
        }
        None => {
            MtpDevice::builder()
                .known_devices(KNOWN_TP7)
                .open_first()
                .await
        }
    }
    .map_err(map_mtp_error)
}

pub fn map_mtp_error(error: mtp_rs::Error) -> AppError {
    if error.is_exclusive_access() {
        return AppError::MtpExclusiveAccess {
            message: error.to_string(),
        };
    }

    AppError::Mtp {
        message: error.to_string(),
    }
}

fn switch_tp7_to_mtp_with_retry(
    device: &Tp7Device,
    timeout: Duration,
) -> Result<MidiSwitchReport, AppError> {
    let deadline = Instant::now() + timeout;

    loop {
        match switch_tp7_to_mtp(device) {
            Ok(report) => return Ok(report),
            Err(error) if is_transient_midi_error(&error) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(250));
            }
            Err(error) => return Err(error),
        }
    }
}

fn wait_for_mtp(
    initial_device: &Tp7Device,
    serial: Option<&str>,
    timeout: Duration,
) -> Result<Tp7Device, AppError> {
    let deadline = Instant::now() + timeout;
    let fallback_serial = initial_device.serial_number.as_deref();

    while Instant::now() < deadline {
        let devices = list_tp7_devices()?;
        let devices = match serial.or(fallback_serial) {
            Some(serial) => devices
                .into_iter()
                .filter(|device| device.serial_number.as_deref() == Some(serial))
                .collect::<Vec<_>>(),
            None => devices,
        };

        if let Some(device) = devices.into_iter().find(is_mtp_visible) {
            return Ok(device);
        }

        thread::sleep(Duration::from_millis(250));
    }

    Err(AppError::MtpNotVisible {
        serial: serial_for_error(initial_device),
        mode: initial_device.mode.to_string(),
    })
}

fn is_mtp_visible(device: &Tp7Device) -> bool {
    matches!(device.mode, UsbMode::Mtp | UsbMode::Mixed)
}

fn is_transient_device_absence(error: &AppError) -> bool {
    matches!(error, AppError::NoDevices | AppError::DeviceNotFound { .. })
}

/// MIDI failures that clear up on their own shortly after the TP-7
/// enumerates: CoreMIDI has not published its endpoints yet, or the device
/// is not answering SysEx yet (seen right after power-on and after replug).
fn is_transient_midi_error(error: &AppError) -> bool {
    match error {
        AppError::Midi { message } => {
            message.contains("CoreMIDI source endpoint")
                || message.contains("CoreMIDI destination endpoint")
        }
        AppError::MidiTimeout { .. } => true,
        _ => false,
    }
}

fn serial_for_error(device: &Tp7Device) -> String {
    device
        .serial_number
        .clone()
        .unwrap_or_else(|| "<no-serial>".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_midi_failures_that_follow_enumeration() {
        let no_source = AppError::Midi {
            message: "No TP-7 CoreMIDI source endpoint was found.".to_string(),
        };
        let silent = AppError::MidiTimeout {
            message: "waiting for MIDI identity response".to_string(),
        };
        let rejected = AppError::Midi {
            message: "unexpected SysEx reply".to_string(),
        };

        assert!(is_transient_midi_error(&no_source));
        assert!(is_transient_midi_error(&silent));
        assert!(!is_transient_midi_error(&rejected));
        assert!(!is_transient_midi_error(&AppError::NoDevices));
    }
}
