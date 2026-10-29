#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::let_underscore_must_use, clippy::undocumented_unsafe_blocks, clippy::unwrap_used)]
fn main() {
    let hashes: Vec<_> = (0..16)
        .map(|seed| {
            let scene = rune_ref::generate::seeded(seed, 8);
            let mut frame = rune_ref::Frame::new(scene.width, scene.height, false);
            frame.render(&scene);
            (seed, frame.hash())
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&hashes).unwrap());
}
