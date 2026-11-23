#![forbid(unsafe_code)]

use rune_ref::{Frame, Scene, generate};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| {
        args.iter()
            .position(|s| s == flag)
            .and_then(|n| args.get(n + 1))
    };
    let scene: Scene = if let Some(path) = value("--scene") {
        serde_json::from_slice(&std::fs::read(path)?)?
    } else {
        generate::seeded(
            value("--seed").map(|s| s.parse()).transpose()?.unwrap_or(0),
            8,
        )
    };
    scene.validate()?;
    let mut frame = Frame::new(scene.width, scene.height, true);
    frame.render(&scene);
    println!("blake3={}", frame.hash());
    if let Some(path) = value("--save") {
        std::fs::write(path, serde_json::to_vec_pretty(&scene)?)?;
    }
    if let Some(xy) = value("--blame") {
        let (x, y) = xy.split_once(',').ok_or("--blame expects x,y")?;
        let (x, y): (u32, u32) = (x.parse()?, y.parse()?);
        if x >= scene.width || y >= scene.height {
            return Err("pixel outside viewport".into());
        }
        match frame.blame(&scene, x, y) {
            Some(p) => println!("node={} source={} kind={}", p.node, p.source, p.params[1]),
            None => println!("clear"),
        }
    }
    if args.iter().any(|a| a == "--diff") {
        #[cfg(not(feature = "gpu"))]
        return Err("--diff requires --features gpu".into());
        #[cfg(feature = "gpu")]
        {
            let mut gpu = rune_ref::gpu::GpuRenderer::default();
            let tolerance = gpu.scene_tolerance(&scene);
            let actual = gpu.render(&scene);
            let report = rune_ref::compare(&scene, &frame, &actual, tolerance);
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.outlier_pixels > 0 {
                return Err("GPU differs from reference".into());
            }
        }
    }
    Ok(())
}
