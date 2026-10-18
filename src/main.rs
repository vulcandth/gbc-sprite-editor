mod app;

use gbc_sprite_editor::{export, project, source};
use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref().is_some_and(|s| s == "--help" || s == "-h") {
        println!(
            "GBC Sprite Editor\n\n  gbc-sprite-editor [IMAGE.png | PROJECT.gbcs]\n  gbc-sprite-editor --export PROJECT.gbcs OUTPUT_DIR [--object-height 8|16] [--tiles-per-row N]\n\nExports require a new or empty output directory. No network connection is needed."
        );
        return Ok(());
    }
    if first.as_deref().is_some_and(|s| s == "--export") {
        let input = PathBuf::from(args.next().ok_or("Missing project path")?);
        let output = PathBuf::from(args.next().ok_or("Missing export directory")?);
        let mut options = export::ExportOptions::default();
        while let Some(flag) = args.next() {
            let value = args.next().ok_or("Missing export option value")?;
            let number = value
                .to_str()
                .ok_or("Invalid numeric option")?
                .parse::<u32>()
                .map_err(|_| "Invalid numeric option")?;
            match flag.to_str() {
                Some("--object-height") => options.object_height = number,
                Some("--tiles-per-row") => options.tiles_per_row = number,
                _ => return Err(format!("Unknown argument: {}", flag.to_string_lossy())),
            }
        }
        let project = project::load(&input)?;
        let image = source::load_png(&project::resolve_image(&project, Some(&input)))?;
        let result = export::export_project(&project, &image, &output, options)?;
        println!(
            "Exported {} slices, {} objects, {} tiles to {}",
            result.slices,
            result.objects,
            result.tiles,
            output.display()
        );
        for warning in result.warnings {
            eprintln!("Warning: {warning}");
        }
        return Ok(());
    }
    if args.next().is_some() {
        return Err("Expected one PNG or project path; use --help".into());
    }
    if first
        .as_ref()
        .is_some_and(|p| p.to_string_lossy().starts_with('-'))
    {
        return Err("Unknown option; use --help".into());
    }
    let initial = first.map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 840.0])
            .with_min_inner_size([880.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "GBC Sprite Editor",
        options,
        Box::new(move |cc| Ok(Box::new(app::Editor::new(cc, initial)))),
    )
    .map_err(|e| e.to_string())
}
