//! cargo run -p deck-system --example logind : takes a delay inhibitor for 3 s and prints InhibitDelayMaxUSec + signals.
fn main() {
    use deck_system::logind::*;
    println!("InhibitDelayMax = {:?}", inhibit_delay_max());
    let _w = watch(|e| println!("signal {e:?}")).map_err(|e| println!("watch: {e}")).ok();
    let mut inh = inhibit_delay("shutdown:sleep", "DeckShell-test", "logind example").expect("inhibit");
    println!("lock held: {}", inh.is_held());
    std::thread::sleep(std::time::Duration::from_secs(3));
    inh.release();
    println!("released");
}
