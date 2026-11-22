fn main() {
    use deck_system::*;
    println!("battery: {:?}", battery());
    println!("backlight raw (raw, max): {:?}", backlight_raw());
    println!("volume: {:?}", volume());
    println!("wifi_enabled: {:?}", wifi_enabled());
    println!("wifi_networks: {:#?}", wifi_networks());
    println!("bluetooth_enabled: {:?}", bluetooth_enabled());
    println!("bluetooth_devices: {:#?}", bluetooth_devices());
    println!("performance_profiles: {:?}", performance_profiles());
    println!("performance_profile: {:?}", performance_profile());
    println!("tdp_limit (cur,min,max): {:?}", tdp_limit());
    println!("gpu_clock: {:?}", gpu_clock());
    println!("battery_charge_limit: {:?}", battery_charge_limit());
    // setters: only no-op writes of the current value
    if let Some((v, _)) = volume() { println!("set_volume(same): {:?}", set_volume(v)); }
}
