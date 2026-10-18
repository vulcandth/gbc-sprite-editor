//! Original procedural pixel art for this project; no external artwork or font.
//! Rebuild with `cargo run --example generate_demo -- examples`.
//! Each robot uses separate steel and lamp palettes, deliberately placed away
//! from a shared grid. Head/body layers overlap but have disjoint RGB masks.

use std::path::PathBuf;

use gbc_sprite_editor::{
    export,
    model::{Cel, Project, Slice, Target},
    project,
};
use image::{Rgba, RgbaImage};

const STEEL: [[u8; 3]; 3] = [[29, 42, 59], [73, 109, 132], [173, 216, 222]];
const LAMPS: [[u8; 3]; 3] = [[255, 211, 105], [247, 126, 83], [234, 250, 237]];

fn block(image: &mut RgbaImage, position: (i32, i32), size: (u32, u32), color: [u8; 3]) {
    let [r, g, b] = color;
    for y in 0..size.1 {
        for x in 0..size.0 {
            image.put_pixel(
                position.0 as u32 + x,
                position.1 as u32 + y,
                Rgba([r, g, b, 255]),
            );
        }
    }
}

fn robot(image: &mut RgbaImage, position: (i32, i32), pose: usize) {
    let (px, py) = position;
    let mut rect =
        |x, y, width, height, color| block(image, (px + x, py + y), (width, height), color);
    // Head: beveled metal shell, lamps, and a one-pixel mouth.
    rect(6, 2, 16, 8, STEEL[0]);
    rect(7, 3, 14, 6, STEEL[1]);
    rect(8, 3, 12, 1, STEEL[2]);
    rect(9, 5, 3, if pose == 1 { 1 } else { 2 }, LAMPS[2]);
    rect(16, 5, 3, 2, LAMPS[2]);
    rect(12, 8, 4, 1, STEEL[0]);
    // Neck, torso, control panel, and legs.
    rect(11, 10, 6, 2, STEEL[0]);
    rect(6, 12, 16, 11, STEEL[0]);
    rect(7, 13, 14, 9, STEEL[1]);
    rect(8, 13, 12, 1, STEEL[2]);
    rect(10, 16, 8, 5, STEEL[0]);
    rect(11, 17, 2, 2, LAMPS[0]);
    rect(15, 17, 2, 2, LAMPS[1]);
    rect(11, 20, 6, 1, LAMPS[2]);
    rect(8, 23, 4, 3, STEEL[0]);
    rect(16, 23, 4, 3, STEEL[0]);
    rect(9, 23, 2, 2, STEEL[2]);
    rect(17, 23, 2, 2, STEEL[2]);
    // Arms are in their own non-grid 8x16 rectangles.
    rect(2, 13, 4, 10, STEEL[0]);
    rect(3, 14, 2, 7, STEEL[1]);
    rect(3, 14, 1, 3, STEEL[2]);
    if pose == 2 {
        rect(22, 13, 5, 4, STEEL[0]);
        rect(24, 5, 4, 10, STEEL[0]);
        rect(25, 6, 2, 9, STEEL[1]);
        rect(23, 4, 6, 4, STEEL[0]);
        rect(24, 5, 4, 2, STEEL[2]);
    } else {
        rect(22, 13, 4, 10, STEEL[0]);
        rect(23, 14, 2, 7, STEEL[1]);
        rect(23, 14, 1, 3, STEEL[2]);
    }
    // Independent 8x8 foot slices; the stepping pose shifts the highlights.
    rect(6, 26, 8, if pose == 1 { 3 } else { 5 }, STEEL[0]);
    rect(7, 26, 6, 2, STEEL[2]);
    rect(14, 26, 8, if pose == 1 { 6 } else { 5 }, STEEL[0]);
    rect(15, if pose == 1 { 28 } else { 26 }, 6, 2, STEEL[2]);
}

fn cel(name: &str, position: (i32, i32), pose: usize) -> Cel {
    let (px, py) = position;
    let mut slices = Vec::new();
    for (label, x, y, height) in [
        ("head-left", 6, 2, 8),
        ("head-right", 14, 2, 8),
        ("body-left", 6, 10, 16),
        ("body-right", 14, 10, 16),
    ] {
        for (layer, palette, colors) in [("steel", 0, STEEL), ("lamps", 1, LAMPS)] {
            slices.push(Slice {
                name: format!("{label}-{layer}"),
                x: px + x,
                y: py + y,
                width: 8,
                height,
                palette,
                colors: Some(colors.to_vec()),
            });
        }
    }
    for (name, x, y, height) in [
        ("arm-left", -2, 10, 16),
        ("arm-right", 22, if pose == 2 { 4 } else { 10 }, 16),
        ("foot-left", 6, 26, 8),
        ("foot-right", 14, 26, 8),
    ] {
        slices.push(Slice {
            name: name.into(),
            x: px + x,
            y: py + y,
            width: 8,
            height,
            palette: 0,
            colors: None,
        });
    }
    Cel {
        name: name.into(),
        origin_x: px + 14,
        origin_y: py + 33,
        slices,
    }
}

fn main() -> Result<(), String> {
    let destination = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| "examples".into());
    std::fs::create_dir_all(&destination).map_err(|e| e.to_string())?;
    let mut image = RgbaImage::new(128, 48);
    let mut cels = Vec::new();
    for (pose, (name, position)) in [("idle", (7, 7)), ("step", (45, 10)), ("wave", (83, 5))]
        .into_iter()
        .enumerate()
    {
        robot(&mut image, position, pose);
        cels.push(cel(name, position, pose));
    }
    let png_path = destination.join("robot.png");
    image.save(&png_path).map_err(|e| e.to_string())?;
    let project = Project {
        image_path: png_path
            .canonicalize()
            .map_err(|e| e.to_string())?
            .to_str()
            .ok_or("Example image path must be UTF-8")?
            .into(),
        target: Target::Cgb,
        transparent_color: None,
        cels,
    };
    // Keep the sample honest: its masked slices must reconstruct every source
    // pixel, including the raised hand and all three accent colors.
    let mut reconstructed = RgbaImage::new(image.width(), image.height());
    for cel in &project.cels {
        for slice in &cel.slices {
            let selected = export::slice_image(&image, slice, project.transparent_color);
            for (x, y, pixel) in selected.enumerate_pixels() {
                if pixel[3] != 0 {
                    reconstructed.put_pixel(slice.x as u32 + x, slice.y as u32 + y, *pixel);
                }
            }
        }
    }
    if reconstructed != image {
        return Err("Example slices must reconstruct every source pixel".into());
    }
    for object_height in [8, 16] {
        let warnings = export::warnings(&project, &image, object_height);
        if !warnings.is_empty() {
            return Err(format!(
                "Example must remain hardware-compatible: {}",
                warnings.join("; ")
            ));
        }
    }
    project::save(&project, &destination.join("robot.gbcs"))?;
    println!(
        "Wrote original procedural robot.png and robot.gbcs to {}",
        destination.display()
    );
    Ok(())
}
