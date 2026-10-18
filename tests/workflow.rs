use gbc_sprite_editor::{
    export::{self, ExportOptions},
    model::{Cel, Project, Slice},
    project, source,
};
use image::{Rgba, RgbaImage};
use std::path::Path;

fn records(path: &Path) -> Vec<csv::StringRecord> {
    csv::Reader::from_path(path)
        .unwrap()
        .records()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn png_to_portable_project_to_layered_tiles_and_layout() {
    let directory = tempfile::tempdir().unwrap();
    let assets = directory.path().join("assets");
    let projects = directory.path().join("projects");
    std::fs::create_dir(&assets).unwrap();
    std::fs::create_dir(&projects).unwrap();
    let png = assets.join("hero.png");
    let red = [224, 64, 72];
    let blue = [64, 112, 216];
    let mut artwork = RgbaImage::from_pixel(32, 32, Rgba([255, 0, 255, 255]));
    for y in 5..21 {
        for x in 3..11 {
            let rgb = if x < 7 { red } else { blue };
            artwork.put_pixel(x, y, Rgba([rgb[0], rgb[1], rgb[2], 255]));
        }
    }
    artwork.save(&png).unwrap();
    let original_png = std::fs::read(&png).unwrap();
    let first = Slice {
        name: "body".into(),
        x: 3,
        y: 5,
        palette: 1,
        colors: Some(vec![red]),
        ..Slice::default()
    };
    let second = Slice {
        name: "clothes".into(),
        palette: 2,
        colors: Some(vec![blue]),
        ..first.clone()
    };
    let definition = Project {
        image_path: dunce::canonicalize(&png).unwrap().to_str().unwrap().into(),
        transparent_color: Some([255, 0, 255]),
        cels: vec![
            Cel {
                name: "idle".into(),
                origin_x: 7,
                origin_y: 17,
                slices: vec![first, second],
            },
            Cel {
                name: "hidden layer".into(),
                origin_x: 20,
                origin_y: 4,
                slices: vec![Slice {
                    name: "hidden".into(),
                    x: 20,
                    y: 4,
                    height: 8,
                    colors: Some(vec![]),
                    ..Slice::default()
                }],
            },
        ],
        ..Project::default()
    };
    let project_file = projects.join("hero.gbcs");
    project::save(&definition, &project_file).unwrap();
    assert!(
        std::fs::read_to_string(&project_file)
            .unwrap()
            .contains("image,../assets/hero.png")
    );
    let restored = project::load(&project_file).unwrap();
    assert_eq!(restored, definition);
    let decoded =
        source::load_png(&project::resolve_image(&restored, Some(&project_file))).unwrap();
    let output = directory.path().join("export");
    let summary = export::export_project(
        &restored,
        &decoded,
        &output,
        ExportOptions {
            object_height: 16,
            tiles_per_row: 3,
        },
    )
    .unwrap();
    assert_eq!((summary.slices, summary.objects, summary.tiles), (3, 3, 6));
    assert!(summary.warnings.is_empty());
    for (index, rgb) in [red, blue].into_iter().enumerate() {
        let layer = source::load_png(&output.join(format!("slices/0000-{index:04}.png"))).unwrap();
        assert_eq!(layer.dimensions(), (8, 16));
        assert_eq!(layer.pixels().filter(|pixel| pixel[3] > 0).count(), 64);
        assert!(
            layer
                .pixels()
                .filter(|pixel| pixel[3] > 0)
                .all(|pixel| pixel.0 == [rgb[0], rgb[1], rgb[2], 255])
        );
    }
    let hidden = source::load_png(&output.join("slices/0001-0000.png")).unwrap();
    assert!(hidden.pixels().all(|pixel| pixel[3] == 0));
    let objects = records(&output.join("objects.csv"));
    assert_eq!(
        (
            &objects[0][7],
            &objects[0][8],
            &objects[0][9],
            &objects[0][10]
        ),
        ("-4", "-12", "0", "1")
    );
    assert_eq!(
        (
            &objects[1][7],
            &objects[1][8],
            &objects[1][9],
            &objects[1][10]
        ),
        ("-4", "-12", "2", "2")
    );
    assert_eq!(
        (&objects[2][7], &objects[2][8], &objects[2][9]),
        ("0", "0", "4")
    );
    assert_eq!(records(&output.join("cels.csv")).len(), 2);
    let tiles = source::load_png(&output.join("tiles.png")).unwrap();
    assert_eq!(tiles.dimensions(), (24, 16));
    assert_eq!(std::fs::read(&png).unwrap(), original_png);
}

#[test]
fn reloading_external_png_changes_exports_without_changing_slices() {
    let directory = tempfile::tempdir().unwrap();
    let png = directory.path().join("sprite.png");
    let file = directory.path().join("sprite.gbcs");
    let initial = RgbaImage::from_pixel(8, 8, Rgba([8, 16, 24, 255]));
    initial.save(&png).unwrap();
    let mut definition = Project {
        image_path: dunce::canonicalize(&png).unwrap().to_str().unwrap().into(),
        ..Project::default()
    };
    definition.cels[0].slices.push(Slice {
        height: 8,
        ..Slice::default()
    });
    project::save(&definition, &file).unwrap();
    let restored = project::load(&file).unwrap();
    let options = ExportOptions {
        object_height: 8,
        tiles_per_row: 1,
    };
    let old_source = source::load_png(Path::new(&restored.image_path)).unwrap();
    export::export_project(
        &restored,
        &old_source,
        &directory.path().join("before"),
        options,
    )
    .unwrap();
    let updated = RgbaImage::from_pixel(8, 8, Rgba([32, 64, 128, 255]));
    updated.save(&png).unwrap();
    let new_source = source::load_png(Path::new(&restored.image_path)).unwrap();
    export::export_project(
        &restored,
        &new_source,
        &directory.path().join("after"),
        options,
    )
    .unwrap();
    assert_eq!(restored, project::load(&file).unwrap());
    let before = source::load_png(&directory.path().join("before/tiles.png")).unwrap();
    let after = source::load_png(&directory.path().join("after/tiles.png")).unwrap();
    assert_eq!(before, initial);
    assert_eq!(after, updated);
}
