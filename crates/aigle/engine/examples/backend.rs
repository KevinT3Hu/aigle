//! Print the selected compute backend: `cargo run -p aigle-engine --example backend`.
fn main() {
    let e = aigle_engine::create(Default::default());
    let (kind, why) = e.backend();
    println!("{kind:?}: {why}");
}
