//! Lists every USB configuration the TP-7 offers, with its interfaces.
use nusb::MaybeFuture;

fn main() {
    let devices = nusb::list_devices().wait().expect("enumerate");
    for info in devices.filter(|d| d.vendor_id() == 0x2367) {
        println!(
            "{:04x}:{:04x} {:?} serial={:?}",
            info.vendor_id(),
            info.product_id(),
            info.product_string(),
            info.serial_number()
        );
        let device = match info.open().wait() {
            Ok(device) => device,
            Err(error) => {
                println!("  open failed: {error}");
                continue;
            }
        };
        for index in 1..=8u8 {
            match device
                .get_string_descriptor(
                    std::num::NonZero::new(index).unwrap(),
                    0x0409,
                    std::time::Duration::from_secs(1),
                )
                .wait()
            {
                Ok(text) => println!("  string {index}: {text:?}"),
                Err(error) => println!("  string {index}: {error}"),
            }
        }
        match device.claim_interface(0).wait() {
            Ok(_iface) => println!("  claim interface 0: ok"),
            Err(error) => println!("  claim interface 0: {error}"),
        }
        match device.detach_and_claim_interface(0).wait() {
            Ok(_iface) => println!("  detach+claim interface 0: ok"),
            Err(error) => println!("  detach+claim interface 0: {error}"),
        }
        println!(
            "  active configuration: {:?}",
            device
                .active_configuration()
                .map(|c| c.configuration_value())
        );
        for config in device.configurations() {
            println!(
                "  configuration {} ({:?}):",
                config.configuration_value(),
                config.string_index()
            );
            for interface in config.interfaces() {
                for alt in interface.alt_settings() {
                    println!(
                        "    iface {} alt {} class {:02x}/{:02x}/{:02x} endpoints {} name idx {:?}",
                        alt.interface_number(),
                        alt.alternate_setting(),
                        alt.class(),
                        alt.subclass(),
                        alt.protocol(),
                        alt.endpoints().count(),
                        alt.string_index()
                    );
                }
            }
        }
    }
}
