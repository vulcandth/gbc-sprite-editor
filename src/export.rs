//! Deterministic PNG and CSV exports for existing Game Boy tile toolchains.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

use image::{Rgba, RgbaImage};

use crate::model::{MAX_SLICE_DIMENSION, Project, Slice, Target};

const MAX_TILES: usize = 65_536;
const MAX_WARNINGS: usize = 200;

#[derive(Clone, Copy, Debug)]
pub struct ExportOptions {
    pub object_height: u32,
    pub tiles_per_row: u32,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            object_height: 16,
            tiles_per_row: 16,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExportSummary {
    pub tiles: usize,
    pub objects: usize,
    pub slices: usize,
    pub warnings: Vec<String>,
}

/// Extract a slice, retaining source alpha and padding out-of-image pixels.
/// A color mask or transparent-color key removes pixels by exact RGB match.
/// Invalid dimensions above the project's 256-pixel limit return an empty image.
pub fn slice_image(
    source: &RgbaImage,
    slice: &Slice,
    transparent_color: Option<[u8; 3]>,
) -> RgbaImage {
    if slice.width > MAX_SLICE_DIMENSION || slice.height > MAX_SLICE_DIMENSION {
        return RgbaImage::new(0, 0);
    }
    let allowed_colors = slice
        .colors
        .as_ref()
        .map(|colors| colors.iter().copied().collect::<HashSet<_>>());
    let mut result = RgbaImage::new(slice.width, slice.height);
    for y in 0..slice.height {
        for x in 0..slice.width {
            let sx = i64::from(slice.x) + i64::from(x);
            let sy = i64::from(slice.y) + i64::from(y);
            if sx < 0
                || sy < 0
                || sx >= i64::from(source.width())
                || sy >= i64::from(source.height())
            {
                continue;
            }
            let pixel = source.get_pixel(sx as u32, sy as u32);
            let rgb = [pixel[0], pixel[1], pixel[2]];
            if pixel[3] == 0
                || transparent_color == Some(rgb)
                || allowed_colors
                    .as_ref()
                    .is_some_and(|colors| !colors.contains(&rgb))
            {
                continue;
            }
            result.put_pixel(x, y, *pixel);
        }
    }
    result
}

fn counts(project: &Project, object_height: u32) -> Result<(usize, usize, usize), String> {
    project.validate()?;
    if !matches!(object_height, 8 | 16) {
        return Err("Object height must be 8 or 16 pixels.".into());
    }
    let mut objects = 0usize;
    let mut slices = 0usize;
    for cel in &project.cels {
        for slice in &cel.slices {
            let count = (slice.width.div_ceil(8) * slice.height.div_ceil(object_height)) as usize;
            objects = objects
                .checked_add(count)
                .ok_or("Too many objects to export.")?;
            slices = slices.checked_add(1).ok_or("Too many slices to export.")?;
            if objects > MAX_TILES / (object_height as usize / 8) {
                return Err(format!(
                    "Export exceeds {MAX_TILES} tiles. Export fewer cels or slices at once."
                ));
            }
        }
    }
    Ok((objects * (object_height as usize / 8), objects, slices))
}

fn add_warning(result: &mut Vec<String>, text: String) {
    if result.len() < MAX_WARNINGS {
        result.push(text);
    } else if result.len() == MAX_WARNINGS {
        result
            .push("Additional warnings omitted; resolve earlier warnings and check again.".into());
    }
}

/// Advisory hardware checks. PNG data is never quantized or silently recolored.
pub fn warnings(project: &Project, image: &RgbaImage, object_height: u32) -> Vec<String> {
    if let Err(error) = counts(project, object_height) {
        return vec![error];
    }
    let mut result = Vec::new();
    for (cel_index, cel) in project.cels.iter().enumerate() {
        let mut events = BTreeMap::<i64, i32>::new();
        let mut palette_colors = BTreeMap::<u8, HashSet<[u8; 3]>>::new();
        let mut cel_objects = 0usize;
        for (slice_index, slice) in cel.slices.iter().enumerate() {
            let label = format!(
                "Cel {} ({:?}), slice {} ({:?})",
                cel_index, cel.name, slice_index, slice.name
            );
            if slice.x < 0
                || slice.y < 0
                || i64::from(slice.x) + i64::from(slice.width) > i64::from(image.width())
                || i64::from(slice.y) + i64::from(slice.height) > i64::from(image.height())
            {
                add_warning(
                    &mut result,
                    format!("{label}: extends outside the PNG; missing pixels are transparent."),
                );
            }
            if matches!(project.target, Target::Dmg) && slice.palette > 1 {
                add_warning(
                    &mut result,
                    format!(
                        "{label}: DMG supports only OBJ palettes 0 and 1 (selected {}).",
                        slice.palette
                    ),
                );
            }
            let pixels = slice_image(image, slice, project.transparent_color);
            let mut too_many_colors = 0usize;
            let mut partial_alpha = false;
            for oy in (0..slice.height).step_by(object_height as usize) {
                for ox in (0..slice.width).step_by(8) {
                    cel_objects += 1;
                    let top = i64::from(slice.y) + i64::from(oy);
                    *events.entry(top).or_default() += 1;
                    *events.entry(top + i64::from(object_height)).or_default() -= 1;
                    let mut colors = HashSet::new();
                    for y in oy..(oy + object_height).min(slice.height) {
                        for x in ox..(ox + 8).min(slice.width) {
                            let pixel = pixels.get_pixel(x, y);
                            if pixel[3] != 0 {
                                let rgb = [pixel[0], pixel[1], pixel[2]];
                                if colors.len() < 4 {
                                    colors.insert(rgb);
                                }
                                let shared_colors =
                                    palette_colors.entry(slice.palette).or_default();
                                if shared_colors.len() < 4 {
                                    shared_colors.insert(rgb);
                                }
                                partial_alpha |= pixel[3] != 255;
                            }
                        }
                    }
                    too_many_colors += usize::from(colors.len() > 3);
                }
            }
            if too_many_colors != 0 {
                add_warning(
                    &mut result,
                    format!(
                        "{label}: {too_many_colors} hardware object(s) use more than 3 visible RGB colors; GB/C OBJ reserves color 0 for transparency."
                    ),
                );
            }
            if partial_alpha {
                add_warning(
                    &mut result,
                    format!(
                        "{label}: partial alpha is preserved in PNG output, but GB/C hardware only supports opaque or transparent pixels."
                    ),
                );
            }
        }
        for (palette, colors) in palette_colors {
            if colors.len() > 3 {
                add_warning(
                    &mut result,
                    format!(
                        "Cel {cel_index} ({:?}): objects assigned to palette {palette} collectively use more than 3 visible RGB colors. One hardware OBJ palette has 3 visible colors.",
                        cel.name
                    ),
                );
            }
        }
        if cel_objects > 40 {
            add_warning(
                &mut result,
                format!(
                    "Cel {cel_index} ({:?}) needs {cel_objects} objects; GB/C supports 40 objects on screen.",
                    cel.name
                ),
            );
        }
        let mut active = 0i32;
        let mut peak = 0i32;
        for delta in events.values() {
            active += delta;
            peak = peak.max(active);
        }
        if peak > 10 {
            add_warning(
                &mut result,
                format!(
                    "Cel {cel_index} ({:?}) has up to {peak} objects crossing a scanline; GB/C displays at most 10.",
                    cel.name
                ),
            );
        }
    }
    result
}

fn check_destination(destination: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(
                    "Export destination must be a new or empty directory, not a file or symlink."
                        .into(),
                );
            }
            if fs::read_dir(destination)
                .map_err(|e| format!("Cannot read export destination: {e}"))?
                .next()
                .is_some()
            {
                return Err("Export destination is not empty. Choose a new folder; existing files will not be overwritten.".into());
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Cannot inspect export destination: {error}")),
    }
}

fn csv_writer(path: &Path, headers: &[&str]) -> Result<csv::Writer<fs::File>, String> {
    let mut writer = csv::Writer::from_path(path).map_err(|e| format!("Cannot create CSV: {e}"))?;
    writer
        .write_record(headers)
        .map_err(|e| format!("Cannot write CSV header: {e}"))?;
    Ok(writer)
}

fn row(writer: &mut csv::Writer<fs::File>, cells: &[String]) -> Result<(), String> {
    writer
        .write_record(cells)
        .map_err(|e| format!("Cannot write CSV: {e}"))
}

fn mask_name(slice: &Slice) -> &'static str {
    match slice.colors.as_ref() {
        None => "all",
        Some(colors) if colors.is_empty() => "none",
        Some(_) => "allowlist",
    }
}

/// Stage the entire export next to its destination, then publish the directory.
/// Existing files and source PNGs are never overwritten.
pub fn export_project(
    project: &Project,
    image: &RgbaImage,
    destination: &Path,
    options: ExportOptions,
) -> Result<ExportSummary, String> {
    let (tile_count, object_count, slice_count) = counts(project, options.object_height)?;
    if tile_count == 0 {
        return Err("Add at least one slice before exporting.".into());
    }
    if !(1..=256).contains(&options.tiles_per_row) {
        return Err("Tiles per row must be between 1 and 256.".into());
    }
    let atlas_rows = (tile_count as u32).div_ceil(options.tiles_per_row);
    let atlas_height = atlas_rows * 8;
    if atlas_height > 65_536 {
        return Err(
            "Tile atlas would be too tall. Increase tiles per row or export fewer slices.".into(),
        );
    }
    let destination = if destination.is_absolute() {
        destination.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("Cannot resolve export path: {e}"))?
            .join(destination)
    };
    if destination.file_name().is_none() {
        return Err("Choose a named directory for the export.".into());
    }
    check_destination(&destination)?;
    let parent = destination
        .parent()
        .ok_or("Export directory needs a parent directory.")?;
    let staging = tempfile::Builder::new()
        .prefix(".gbc-export-")
        .tempdir_in(parent)
        .map_err(|e| {
            format!("Cannot stage export beside destination (the parent must exist): {e}")
        })?;
    let root = staging.path();
    fs::create_dir(root.join("slices"))
        .map_err(|e| format!("Cannot create slices directory: {e}"))?;

    let mut objects_csv = csv_writer(
        &root.join("objects.csv"),
        &[
            "object_index",
            "cel_index",
            "cel_name",
            "slice_index",
            "slice_name",
            "source_x",
            "source_y",
            "x",
            "y",
            "tile_index",
            "palette",
            "width",
            "height",
        ],
    )?;
    let mut slices_csv = csv_writer(
        &root.join("slices.csv"),
        &[
            "cel_index",
            "cel_name",
            "slice_index",
            "slice_name",
            "source_x",
            "source_y",
            "width",
            "height",
            "origin_x",
            "origin_y",
            "offset_x",
            "offset_y",
            "palette",
            "first_tile_index",
            "tile_count",
            "first_object_index",
            "object_count",
            "color_mask",
            "png",
        ],
    )?;
    let mut cels_csv = csv_writer(
        &root.join("cels.csv"),
        &[
            "cel_index",
            "cel_name",
            "origin_x",
            "origin_y",
            "first_object_index",
            "object_count",
            "first_tile_index",
            "tile_count",
        ],
    )?;
    let mut palettes_csv = csv_writer(
        &root.join("palettes.csv"),
        &[
            "cel_index",
            "cel_name",
            "slice_index",
            "slice_name",
            "palette",
            "mask",
            "color_index",
            "r",
            "g",
            "b",
        ],
    )?;

    let mut atlas = RgbaImage::new(options.tiles_per_row * 8, atlas_height);
    let mut tile_index = 0usize;
    let mut object_index = 0usize;
    for (cel_index, cel) in project.cels.iter().enumerate() {
        let cel_first_tile = tile_index;
        let cel_first_object = object_index;
        for (slice_index, slice) in cel.slices.iter().enumerate() {
            let first_tile = tile_index;
            let first_object = object_index;
            let pixels = slice_image(image, slice, project.transparent_color);
            let png_name = format!("slices/{cel_index:04}-{slice_index:04}.png");
            pixels
                .save_with_format(root.join(&png_name), image::ImageFormat::Png)
                .map_err(|e| format!("Cannot save slice PNG: {e}"))?;
            for oy in (0..slice.height).step_by(options.object_height as usize) {
                for ox in (0..slice.width).step_by(8) {
                    let source_x = i64::from(slice.x) + i64::from(ox);
                    let source_y = i64::from(slice.y) + i64::from(oy);
                    row(
                        &mut objects_csv,
                        &[
                            object_index.to_string(),
                            cel_index.to_string(),
                            cel.name.clone(),
                            slice_index.to_string(),
                            slice.name.clone(),
                            source_x.to_string(),
                            source_y.to_string(),
                            (source_x - i64::from(cel.origin_x)).to_string(),
                            (source_y - i64::from(cel.origin_y)).to_string(),
                            tile_index.to_string(),
                            slice.palette.to_string(),
                            "8".into(),
                            options.object_height.to_string(),
                        ],
                    )?;
                    for tile_y in (0..options.object_height).step_by(8) {
                        let atlas_x = (tile_index as u32 % options.tiles_per_row) * 8;
                        let atlas_y = (tile_index as u32 / options.tiles_per_row) * 8;
                        for y in 0..8 {
                            for x in 0..8 {
                                let sx = ox + x;
                                let sy = oy + tile_y + y;
                                let pixel = if sx < pixels.width() && sy < pixels.height() {
                                    *pixels.get_pixel(sx, sy)
                                } else {
                                    Rgba([0, 0, 0, 0])
                                };
                                atlas.put_pixel(atlas_x + x, atlas_y + y, pixel);
                            }
                        }
                        tile_index += 1;
                    }
                    object_index += 1;
                }
            }
            row(
                &mut slices_csv,
                &[
                    cel_index.to_string(),
                    cel.name.clone(),
                    slice_index.to_string(),
                    slice.name.clone(),
                    slice.x.to_string(),
                    slice.y.to_string(),
                    slice.width.to_string(),
                    slice.height.to_string(),
                    cel.origin_x.to_string(),
                    cel.origin_y.to_string(),
                    (i64::from(slice.x) - i64::from(cel.origin_x)).to_string(),
                    (i64::from(slice.y) - i64::from(cel.origin_y)).to_string(),
                    slice.palette.to_string(),
                    first_tile.to_string(),
                    (tile_index - first_tile).to_string(),
                    first_object.to_string(),
                    (object_index - first_object).to_string(),
                    mask_name(slice).into(),
                    png_name,
                ],
            )?;
            let prefix = vec![
                cel_index.to_string(),
                cel.name.clone(),
                slice_index.to_string(),
                slice.name.clone(),
                slice.palette.to_string(),
                mask_name(slice).into(),
            ];
            match slice.colors.as_ref().filter(|colors| !colors.is_empty()) {
                Some(colors) => {
                    for (index, color) in colors.iter().enumerate() {
                        let mut record = prefix.clone();
                        record.extend([
                            index.to_string(),
                            color[0].to_string(),
                            color[1].to_string(),
                            color[2].to_string(),
                        ]);
                        row(&mut palettes_csv, &record)?;
                    }
                }
                None => {
                    let mut record = prefix;
                    record.extend([String::new(), String::new(), String::new(), String::new()]);
                    row(&mut palettes_csv, &record)?;
                }
            }
        }
        row(
            &mut cels_csv,
            &[
                cel_index.to_string(),
                cel.name.clone(),
                cel.origin_x.to_string(),
                cel.origin_y.to_string(),
                cel_first_object.to_string(),
                (object_index - cel_first_object).to_string(),
                cel_first_tile.to_string(),
                (tile_index - cel_first_tile).to_string(),
            ],
        )?;
    }
    for mut writer in [objects_csv, slices_csv, cels_csv, palettes_csv] {
        writer
            .flush()
            .map_err(|e| format!("Cannot finish CSV: {e}"))?;
    }
    atlas
        .save_with_format(root.join("tiles.png"), image::ImageFormat::Png)
        .map_err(|e| format!("Cannot save tile atlas PNG: {e}"))?;
    // Check again after rendering: a user may have placed files in the chosen folder.
    if check_destination(&destination)? {
        fs::remove_dir(&destination)
            .map_err(|e| format!("Export folder changed while exporting: {e}"))?;
    }
    fs::rename(root, &destination).map_err(|e| format!("Cannot publish export directory: {e}"))?;
    Ok(ExportSummary {
        tiles: tile_count,
        objects: object_count,
        slices: slice_count,
        warnings: warnings(project, image, options.object_height),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Cel;

    fn slice(x: i32, y: i32, width: u32, height: u32) -> Slice {
        Slice {
            name: "body, \"left\"".into(),
            x,
            y,
            width,
            height,
            palette: 0,
            colors: None,
        }
    }

    fn project(slices: Vec<Slice>) -> Project {
        Project {
            image_path: "source.png".into(),
            target: Target::Cgb,
            transparent_color: None,
            cels: vec![Cel {
                name: "walk\nleft".into(),
                origin_x: 4,
                origin_y: 8,
                slices,
            }],
        }
    }

    fn records(path: &Path) -> Vec<csv::StringRecord> {
        csv::Reader::from_path(path)
            .unwrap()
            .records()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn mask_key_and_padding_preserve_visible_alpha() {
        let mut source = RgbaImage::new(3, 1);
        source.put_pixel(0, 0, Rgba([255, 0, 0, 127]));
        source.put_pixel(1, 0, Rgba([0, 255, 0, 255]));
        source.put_pixel(2, 0, Rgba([0, 0, 255, 255]));
        let mut selected = slice(-1, 0, 5, 2);
        selected.colors = Some(vec![[255, 0, 0], [0, 255, 0]]);
        let result = slice_image(&source, &selected, Some([0, 255, 0]));
        assert_eq!(result.get_pixel(1, 0).0, [255, 0, 0, 127]);
        assert_eq!(result.pixels().filter(|p| p[3] != 0).count(), 1);
        selected.colors = Some(vec![]);
        assert!(
            slice_image(&source, &selected, None)
                .pixels()
                .all(|p| p[3] == 0)
        );
    }

    #[test]
    fn eight_by_sixteen_pairs_and_signed_offsets_survive_export() {
        let mut source = RgbaImage::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                source.put_pixel(
                    x,
                    y,
                    Rgba([1 + (x / 8) as u8 * 2 + (y / 8) as u8, 0, 0, 255]),
                );
            }
        }
        let project = project(vec![slice(0, 0, 16, 16)]);
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("export");
        let summary = export_project(
            &project,
            &source,
            &output,
            ExportOptions {
                object_height: 16,
                tiles_per_row: 3,
            },
        )
        .unwrap();
        assert_eq!((summary.tiles, summary.objects, summary.slices), (4, 2, 1));
        let atlas = image::open(output.join("tiles.png")).unwrap().to_rgba8();
        for (x, y, color) in [(0, 0, 1), (8, 0, 2), (16, 0, 3), (0, 8, 4)] {
            assert_eq!(atlas.get_pixel(x, y)[0], color);
        }
        let rows = records(&output.join("objects.csv"));
        assert_eq!(&rows[0][2], "walk\nleft");
        assert_eq!(&rows[0][4], "body, \"left\"");
        assert_eq!((&rows[0][7], &rows[0][8], &rows[0][9]), ("-4", "-8", "0"));
        assert_eq!((&rows[1][7], &rows[1][8], &rows[1][9]), ("4", "-8", "2"));
        assert!(output.join("slices/0000-0000.png").is_file());
    }

    #[test]
    fn irregular_slices_pad_last_object_instead_of_sampling_neighbors() {
        let source = RgbaImage::from_pixel(32, 32, Rgba([20, 30, 40, 255]));
        let project = project(vec![slice(3, 5, 9, 9)]);
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("export");
        let summary = export_project(
            &project,
            &source,
            &output,
            ExportOptions {
                object_height: 8,
                tiles_per_row: 4,
            },
        )
        .unwrap();
        assert_eq!(summary.objects, 4);
        let atlas = image::open(output.join("tiles.png")).unwrap().to_rgba8();
        assert_eq!(atlas.get_pixel(24, 0)[3], 255);
        assert_eq!(atlas.get_pixel(25, 0)[3], 0);
        assert_eq!(atlas.get_pixel(24, 1)[3], 0);
        let rows = records(&output.join("objects.csv"));
        assert_eq!((&rows[3][5], &rows[3][6]), ("11", "13"));
    }

    #[test]
    fn exports_never_overwrite_existing_files() {
        let source = RgbaImage::new(8, 8);
        let project = project(vec![slice(0, 0, 8, 8)]);
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("source.png"), b"precious source").unwrap();
        assert!(export_project(&project, &source, temp.path(), ExportOptions::default()).is_err());
        assert_eq!(
            fs::read(temp.path().join("source.png")).unwrap(),
            b"precious source"
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        let empty = temp.path().join("empty");
        fs::create_dir(&empty).unwrap();
        export_project(&project, &source, &empty, ExportOptions::default()).unwrap();
        assert!(export_project(&project, &source, &empty, ExportOptions::default()).is_err());
    }

    #[test]
    fn validation_reports_hardware_limits_and_partial_alpha() {
        let mut source = RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 255]));
        for x in 0..4 {
            source.put_pixel(x, 0, Rgba([x as u8, 1, 2, 127]));
        }
        let project = project(vec![slice(0, 0, 8, 8); 41]);
        let messages = warnings(&project, &source, 8).join("\n");
        assert!(messages.contains("more than 3"));
        assert!(messages.contains("partial alpha"));
        assert!(messages.contains("40 objects"));
        assert!(messages.contains("at most 10"));
    }

    #[test]
    fn touching_object_rows_do_not_double_count_scanline_limit() {
        let source = RgbaImage::new(80, 16);
        let project = project(vec![slice(0, 0, 80, 16)]);
        assert!(
            !warnings(&project, &source, 8)
                .iter()
                .any(|s| s.contains("scanline"))
        );
    }

    #[test]
    fn shared_palette_colors_are_checked_across_objects() {
        let mut source = RgbaImage::new(16, 8);
        for x in 0..16 {
            source.put_pixel(x, 0, Rgba([(x / 4) as u8, 0, 0, 255]));
        }
        let mut project = project(vec![slice(0, 0, 8, 8), slice(8, 0, 8, 8)]);
        let messages = warnings(&project, &source, 8);
        assert!(
            messages
                .iter()
                .any(|s| s.contains("collectively use more than 3"))
        );
        assert!(
            !messages
                .iter()
                .any(|s| s.contains("hardware object(s) use more than 3"))
        );
        project.cels[0].slices[1].palette = 1;
        assert!(warnings(&project, &source, 8).is_empty());
    }

    #[test]
    fn palette_csv_preserves_all_none_and_allowlist_semantics() {
        let source = RgbaImage::from_pixel(8, 8, Rgba([10, 20, 30, 255]));
        let mut project = project(vec![slice(0, 0, 8, 8); 3]);
        project.cels[0].slices[1].colors = Some(vec![]);
        project.cels[0].slices[2].colors = Some(vec![[10, 20, 30]]);
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("export");
        export_project(&project, &source, &output, ExportOptions::default()).unwrap();
        let rows = records(&output.join("palettes.csv"));
        assert_eq!((&rows[0][5], &rows[0][6]), ("all", ""));
        assert_eq!((&rows[1][5], &rows[1][6]), ("none", ""));
        assert_eq!(
            (
                &rows[2][5],
                &rows[2][6],
                &rows[2][7],
                &rows[2][8],
                &rows[2][9]
            ),
            ("allowlist", "0", "10", "20", "30")
        );
        let hidden = image::open(output.join("slices/0000-0001.png"))
            .unwrap()
            .to_rgba8();
        assert!(hidden.pixels().all(|p| p[3] == 0));
    }

    #[test]
    fn oversized_exports_fail_before_creating_output() {
        let source = RgbaImage::new(1, 1);
        let project = project(vec![slice(0, 0, 256, 256); 65]);
        let temp = tempfile::tempdir().unwrap();
        assert!(
            export_project(
                &project,
                &source,
                &temp.path().join("too-big"),
                ExportOptions::default()
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn offsets_keep_the_full_range_of_valid_signed_coordinates() {
        let source = RgbaImage::new(1, 1);
        let limit = crate::model::MAX_COORDINATE;
        let mut project = project(vec![slice(limit, -limit, 8, 8)]);
        project.cels[0].origin_x = -limit;
        project.cels[0].origin_y = limit;
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("export");
        export_project(&project, &source, &output, ExportOptions::default()).unwrap();
        let rows = records(&output.join("objects.csv"));
        assert_eq!((&rows[0][7], &rows[0][8]), ("2000000", "-2000000"));
    }
}
