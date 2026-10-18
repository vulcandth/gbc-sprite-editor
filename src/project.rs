//! Version 1 of the tagged CSV project format.
//!
//! Image paths are relative to the project file on disk and absolute in memory
//! after loading. Saving to a different directory recalculates the relative path.

use crate::model::{Cel, MAX_CELS, MAX_SLICES, Project, Slice, Target};
use csv::{ReaderBuilder, StringRecord, WriterBuilder};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

const MAX_PROJECT_BYTES: u64 = 32 * 1024 * 1024;

/// Load a project without requiring the referenced PNG to exist.
pub fn load(path: &Path) -> Result<Project, String> {
    let absolute = absolute_path(path);
    let file = File::open(&absolute).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    let bytes = file
        .metadata()
        .map_err(|e| format!("Cannot inspect project: {e}"))?
        .len();
    if bytes > MAX_PROJECT_BYTES {
        return Err("Project file exceeds the 32 MiB limit".into());
    }
    let mut reader = ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(file);
    let mut project = Project {
        cels: Vec::new(),
        ..Project::default()
    };
    let mut cels = BTreeMap::<usize, (Cel, u64)>::new();
    let mut slices = Vec::<(usize, Slice, u64)>::new();
    let mut header_seen = false;
    let mut image_seen = false;
    let mut target_seen = false;
    let mut transparent_seen = false;

    for record in reader.records() {
        let record = record.map_err(|e| format!("Invalid project CSV: {e}"))?;
        let line = record.position().map_or(1, |p| p.line());
        let fail = |message: &str| format!("Line {line}: {message}");
        if !header_seen {
            require_fields(&record, 2, line)?;
            if &record[0] != "gbc_sprite_project" || &record[1] != "1" {
                return Err(fail("expected gbc_sprite_project,1 header"));
            }
            header_seen = true;
            continue;
        }
        match record.get(0).unwrap_or("") {
            "image" => {
                require_fields(&record, 2, line)?;
                if image_seen {
                    return Err(fail("duplicate image record"));
                }
                if record[1].contains('\0') {
                    return Err(fail("image path contains a NUL character"));
                }
                project.image_path = record[1].to_string();
                image_seen = true;
            }
            "target" => {
                require_fields(&record, 2, line)?;
                if target_seen {
                    return Err(fail("duplicate target record"));
                }
                project.target = match &record[1] {
                    "dmg" => Target::Dmg,
                    "cgb" => Target::Cgb,
                    _ => return Err(fail("target must be dmg or cgb")),
                };
                target_seen = true;
            }
            "transparent" => {
                require_fields(&record, 2, line)?;
                if transparent_seen {
                    return Err(fail("duplicate transparent record"));
                }
                project.transparent_color = if record[1].is_empty() {
                    None
                } else {
                    Some(parse_color(&record[1]).map_err(|e| fail(&e))?)
                };
                transparent_seen = true;
            }
            "cel" => {
                require_fields(&record, 5, line)?;
                let index = parse::<usize>(&record[1], "cel index", line)?;
                if index >= MAX_CELS {
                    return Err(fail("cel index exceeds project limit"));
                }
                if cels.contains_key(&index) {
                    return Err(fail("duplicate cel index"));
                }
                let cel = Cel {
                    name: record[2].to_string(),
                    origin_x: parse(&record[3], "origin x", line)?,
                    origin_y: parse(&record[4], "origin y", line)?,
                    slices: Vec::new(),
                };
                // Use model validation here so malformed values retain CSV line context.
                Project {
                    cels: vec![cel.clone()],
                    ..Project::default()
                }
                .validate()
                .map_err(|e| fail(&e))?;
                cels.insert(index, (cel, line));
            }
            "slice" => {
                require_fields(&record, 9, line)?;
                if slices.len() >= MAX_SLICES {
                    return Err(fail("too many slices"));
                }
                let index = parse::<usize>(&record[1], "cel index", line)?;
                if index >= MAX_CELS {
                    return Err(fail("cel index exceeds project limit"));
                }
                let colors = match &record[8] {
                    "*" => None,
                    "" => Some(Vec::new()),
                    colors => {
                        if colors.split(';').count() > 256 {
                            return Err(fail("color mask may contain at most 256 colors"));
                        }
                        Some(
                            colors
                                .split(';')
                                .map(parse_color)
                                .collect::<Result<Vec<_>, _>>()
                                .map_err(|e| fail(&e))?,
                        )
                    }
                };
                let slice = Slice {
                    name: record[2].to_string(),
                    x: parse(&record[3], "slice x", line)?,
                    y: parse(&record[4], "slice y", line)?,
                    width: parse(&record[5], "slice width", line)?,
                    height: parse(&record[6], "slice height", line)?,
                    palette: parse(&record[7], "palette", line)?,
                    colors,
                };
                slices.push((index, slice, line));
            }
            tag => return Err(fail(&format!("unknown record type {tag:?}"))),
        }
    }
    if !header_seen {
        return Err("Missing gbc_sprite_project,1 header".into());
    }
    if !image_seen || !target_seen || !transparent_seen {
        return Err("Missing required image, target, or transparent record".into());
    }
    if cels.is_empty() {
        return Err("Project must contain at least one cel record".into());
    }
    for (expected, (&actual, (_, line))) in cels.iter().enumerate() {
        if actual != expected {
            return Err(format!(
                "Line {line}: cel indices must be contiguous from 0 (expected {expected}, found {actual})"
            ));
        }
    }
    for (index, slice, line) in slices {
        let cel = cels
            .get_mut(&index)
            .ok_or_else(|| format!("Line {line}: slice references missing cel {index}"))?;
        Project {
            target: project.target,
            cels: vec![Cel {
                slices: vec![slice.clone()],
                ..Cel::default()
            }],
            ..Project::default()
        }
        .validate()
        .map_err(|e| format!("Line {line}: {e}"))?;
        cel.0.slices.push(slice);
    }
    project.cels = cels.into_values().map(|(cel, _)| cel).collect();
    project.validate()?;
    if !project.image_path.is_empty() {
        project.image_path = path_string(&resolve_image(&project, Some(&absolute)))?;
    }
    Ok(project)
}

/// Save atomically. A relative in-memory image path is interpreted from cwd.
/// Existing project data is left intact if validation or serialization fails.
pub fn save(project: &Project, path: &Path) -> Result<(), String> {
    project.validate()?;
    let destination = absolute_path(path);
    if destination.file_name().is_none() {
        return Err("Project destination must be a file".into());
    }
    let requested_parent = destination
        .parent()
        .ok_or("Project destination has no parent directory")?;
    // Relative image references must use the actual destination directory: a
    // symlinked parent can have a different depth from its apparent path.
    let parent = dunce::canonicalize(requested_parent).map_err(|e| {
        format!(
            "Cannot access project directory {}: {e}",
            requested_parent.display()
        )
    })?;
    let destination = parent.join(destination.file_name().unwrap());
    let image_path = if project.image_path.is_empty() {
        String::new()
    } else {
        let source = resolve_image(project, None);
        let same_source = match (source.canonicalize(), destination.canonicalize()) {
            (Ok(source), Ok(destination)) => source == destination,
            _ => source == destination,
        };
        if same_source {
            return Err("Choose a project filename different from the source PNG".into());
        }
        let relative = pathdiff::diff_paths(&source, &parent).unwrap_or(source);
        // Slash-separated relative paths also load on Windows.
        path_string(&relative)?.replace(std::path::MAIN_SEPARATOR, "/")
    };
    let mut temporary = NamedTempFile::new_in(&parent)
        .map_err(|e| format!("Cannot create project in {}: {e}", parent.display()))?;
    {
        let mut writer = WriterBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_writer(temporary.as_file_mut());
        let mut write = |fields: &[&str]| {
            writer
                .write_record(fields)
                .map_err(|e| format!("Cannot write project CSV: {e}"))
        };
        write(&["gbc_sprite_project", "1"])?;
        write(&["image", &image_path])?;
        write(&[
            "target",
            match project.target {
                Target::Dmg => "dmg",
                Target::Cgb => "cgb",
            },
        ])?;
        write(&[
            "transparent",
            &project
                .transparent_color
                .map(format_color)
                .unwrap_or_default(),
        ])?;
        for (index, cel) in project.cels.iter().enumerate() {
            write(&[
                "cel",
                &index.to_string(),
                &cel.name,
                &cel.origin_x.to_string(),
                &cel.origin_y.to_string(),
            ])?;
            for slice in &cel.slices {
                let colors = match &slice.colors {
                    None => "*".into(),
                    Some(colors) => colors
                        .iter()
                        .copied()
                        .map(format_color)
                        .collect::<Vec<_>>()
                        .join(";"),
                };
                write(&[
                    "slice",
                    &index.to_string(),
                    &slice.name,
                    &slice.x.to_string(),
                    &slice.y.to_string(),
                    &slice.width.to_string(),
                    &slice.height.to_string(),
                    &slice.palette.to_string(),
                    &colors,
                ])?;
            }
        }
        writer
            .flush()
            .map_err(|e| format!("Cannot flush project CSV: {e}"))?;
    }
    temporary
        .as_file_mut()
        .flush()
        .map_err(|e| format!("Cannot flush project file: {e}"))?;
    if temporary
        .as_file()
        .metadata()
        .map_err(|e| format!("Cannot inspect project file: {e}"))?
        .len()
        > MAX_PROJECT_BYTES
    {
        return Err("Project file exceeds the 32 MiB limit".into());
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|e| format!("Cannot sync project file: {e}"))?;
    temporary
        .persist(&destination)
        .map_err(|e| format!("Cannot save {}: {}", path.display(), e.error))?;
    Ok(())
}

/// Resolve a source image relative to its project, or cwd for a new project.
/// An empty source remains empty; the image does not have to exist yet.
pub fn resolve_image(project: &Project, project_path: Option<&Path>) -> PathBuf {
    if project.image_path.is_empty() {
        return PathBuf::new();
    }
    let image = Path::new(&project.image_path);
    if image.is_absolute() {
        return resolve_existing_path(image);
    }
    let base = project_path
        .map(absolute_path)
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    resolve_existing_path(&base.join(image))
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn resolve_existing_path(path: &Path) -> PathBuf {
    if let Ok(resolved) = dunce::canonicalize(path) {
        return resolved;
    }
    // Canonicalize the existing prefix of missing paths, preserving all unknown
    // components (including `..`) after it. This also handles macOS /var aliases
    // and keeps references into not-yet-created asset directories portable.
    let mut ancestor = path.parent();
    while let Some(parent) = ancestor {
        if let Ok(resolved) = dunce::canonicalize(parent)
            && let Ok(suffix) = path.strip_prefix(parent)
        {
            return resolved.join(suffix);
        }
        ancestor = parent.parent();
    }
    // Do not collapse `..` lexically: it may follow a symlink, or a directory
    // that does not exist yet. Preserve its filesystem meaning for relinking.
    path.to_path_buf()
}

fn path_string(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| "Project format requires UTF-8 image paths".into())
}

fn require_fields(record: &StringRecord, count: usize, line: u64) -> Result<(), String> {
    if record.len() == count {
        Ok(())
    } else {
        Err(format!(
            "Line {line}: {:?} record requires {count} fields; found {}",
            record.get(0).unwrap_or(""),
            record.len()
        ))
    }
}

fn parse<T: std::str::FromStr>(value: &str, field: &str, line: u64) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("Line {line}: invalid {field} {value:?}"))
}

fn parse_color(value: &str) -> Result<[u8; 3], String> {
    if value.len() != 6 || !value.is_ascii() || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("Invalid RGB color {value:?}; expected RRGGBB"));
    }
    let mut rgb = [0; 3];
    for (channel, output) in rgb.iter_mut().enumerate() {
        *output = u8::from_str_radix(&value[channel * 2..channel * 2 + 2], 16)
            .map_err(|_| format!("Invalid RGB color {value:?}"))?;
    }
    Ok(rgb)
}

fn format_color(color: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", color[0], color[1], color[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_text(text: &str) -> Result<Project, String> {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.gbcs");
        std::fs::write(&path, text).unwrap();
        load(&path)
    }

    const PREFIX: &str =
        "gbc_sprite_project,1\nimage,source.png\ntarget,cgb\ntransparent,\ncel,0,cel_0,0,0\n";

    #[test]
    fn roundtrip_preserves_text_masks_and_origins() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("画像,quoted\"name.png");
        let mut project = Project {
            image_path: path_string(&resolve_existing_path(&source)).unwrap(),
            transparent_color: Some([255, 0, 255]),
            ..Project::default()
        };
        project.cels[0] = Cel {
            name: "Hero, \"jump\"\n二".into(),
            origin_x: -11,
            origin_y: 23,
            slices: vec![
                Slice {
                    name: "hair, \"front\"\nlayer".into(),
                    colors: None,
                    ..Slice::default()
                },
                Slice {
                    name: "empty".into(),
                    colors: Some(vec![]),
                    ..Slice::default()
                },
                Slice {
                    name: "body".into(),
                    x: -8,
                    y: 9,
                    palette: 7,
                    colors: Some(vec![[1, 2, 3], [254, 128, 64]]),
                    ..Slice::default()
                },
            ],
        };
        let path = directory.path().join("saved.gbcs");
        save(&project, &path).unwrap();
        assert_eq!(load(&path).unwrap(), project);
        // Replacement of an existing file is supported as well.
        project.cels[0].origin_x = 42;
        save(&project, &path).unwrap();
        assert_eq!(load(&path).unwrap(), project);
    }

    #[test]
    fn save_as_preserves_image_reference_and_relocation_is_portable() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        let relocated = directory.path().join("moved");
        std::fs::create_dir_all(first.join("assets")).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let project = Project {
            image_path: path_string(&resolve_existing_path(&first.join("assets/sprite.png")))
                .unwrap(),
            ..Project::default()
        };
        let first_file = first.join("scene.gbcs");
        save(&project, &first_file).unwrap();
        let text = std::fs::read_to_string(&first_file).unwrap();
        assert!(text.contains("image,assets/sprite.png"));
        let loaded = load(&first_file).unwrap();
        let second_file = second.join("scene.gbcs");
        save(&loaded, &second_file).unwrap();
        assert_eq!(load(&second_file).unwrap().image_path, project.image_path);
        assert!(
            std::fs::read_to_string(&second_file)
                .unwrap()
                .contains("image,../first/assets/sprite.png")
        );
        std::fs::rename(&first, &relocated).unwrap();
        assert_eq!(
            load(&relocated.join("scene.gbcs")).unwrap().image_path,
            path_string(&resolve_existing_path(&relocated.join("assets/sprite.png"))).unwrap()
        );
    }

    #[test]
    fn malformed_records_report_their_line() {
        for tail in [
            "unknown,stuff\n",
            "target,cgb\n",
            "image,another.png\n",
            "transparent,000000\n",
            "cel,0,duplicate,0,0\n",
            "cel,2,gap,0,0\n",
            "slice,9,orphan,0,0,8,16,0,*\n",
            "slice,0,bad,NaN,0,8,16,0,*\n",
            "slice,0,bad,0,0,0,16,0,*\n",
            "slice,0,bad,0,0,8,16,8,*\n",
            "slice,0,bad,0,0,8,16,0,12345G\n",
            "slice,0,bad,0,0,8,16,0,010203;010203\n",
            "slice,0,short,0,0,8,16\n",
        ] {
            let error = read_text(&format!("{PREFIX}{tail}")).unwrap_err();
            assert!(error.contains("Line 6"), "{tail:?}: {error}");
        }
    }

    #[test]
    fn missing_or_unsupported_metadata_is_rejected() {
        for text in [
            "",
            "gbc_sprite_project,2\n",
            "image,x.png\n",
            "gbc_sprite_project,1\ncel,0,a,0,0\n",
        ] {
            assert!(read_text(text).is_err());
        }
        let dmg_with_invalid_palette = format!(
            "{}slice,0,test,0,0,8,16,2,*\n",
            PREFIX.replace("target,cgb", "target,dmg")
        );
        assert!(read_text(&dmg_with_invalid_palette).is_err());
    }

    #[test]
    fn invalid_save_does_not_replace_existing_project() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("project.gbcs");
        let mut project = Project::default();
        save(&project, &path).unwrap();
        let original = std::fs::read(&path).unwrap();
        project.cels.clear();
        assert!(save(&project, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn project_save_never_overwrites_its_source_image() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.png");
        let original = b"preserve source PNG bytes";
        std::fs::write(&path, original).unwrap();
        let project = Project {
            image_path: path.to_str().unwrap().into(),
            ..Project::default()
        };
        assert!(
            save(&project, &path)
                .unwrap_err()
                .contains("different from the source PNG")
        );
        let alias = directory.path().join("unused/../source.png");
        assert!(save(&project, &alias).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn project_save_rejects_symlink_to_source_image() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.png");
        let alias = directory.path().join("alias.gbcs");
        std::fs::write(&source, b"source PNG").unwrap();
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        let project = Project {
            image_path: source.to_str().unwrap().into(),
            ..Project::default()
        };
        assert!(save(&project, &alias).is_err());
        assert!(alias.is_symlink());
        assert_eq!(std::fs::read(source).unwrap(), b"source PNG");
    }

    #[test]
    fn missing_source_resolution_follows_platform_path_semantics() {
        let directory = tempfile::tempdir().unwrap();
        let project = Project {
            image_path: "images/../sprite.png".into(),
            ..Project::default()
        };
        // Windows canonicalization removes `images/..` before filesystem
        // traversal, even if `images` does not exist. Unix must traverse that
        // component first, so our unresolved suffix correctly retains it.
        let missing_suffix = if cfg!(windows) {
            "sprite.png"
        } else {
            "images/../sprite.png"
        };
        assert_eq!(
            resolve_image(&project, Some(&directory.path().join("scene.gbcs"))),
            dunce::canonicalize(directory.path())
                .unwrap()
                .join(missing_suffix)
        );
        std::fs::create_dir(directory.path().join("images")).unwrap();
        assert_eq!(
            resolve_image(&project, Some(&directory.path().join("scene.gbcs"))),
            dunce::canonicalize(directory.path())
                .unwrap()
                .join("sprite.png")
        );
        assert_eq!(resolve_image(&Project::default(), None), PathBuf::new());
    }

    #[cfg(unix)]
    #[test]
    fn source_parent_traversal_follows_symlink_before_dot_dot() {
        let directory = tempfile::tempdir().unwrap();
        let project_dir = directory.path().join("project");
        let shared = directory.path().join("shared");
        std::fs::create_dir(&project_dir).unwrap();
        std::fs::create_dir_all(shared.join("nested")).unwrap();
        std::os::unix::fs::symlink("../shared/nested", project_dir.join("assets_link")).unwrap();
        std::fs::write(shared.join("robot.png"), b"correct PNG").unwrap();
        std::fs::write(project_dir.join("robot.png"), b"wrong PNG").unwrap();
        let path = project_dir.join("robot.gbcs");
        std::fs::write(
            &path,
            PREFIX.replace("source.png", "assets_link/../robot.png"),
        )
        .unwrap();
        let restored = load(&path).unwrap();
        assert_eq!(
            Path::new(&restored.image_path),
            dunce::canonicalize(shared.join("robot.png")).unwrap()
        );
        assert_eq!(std::fs::read(&restored.image_path).unwrap(), b"correct PNG");
        // Even with the image missing, resolving its existing parent must retain
        // the symlink traversal rather than selecting the decoy project image.
        let missing = Project {
            image_path: "assets_link/../missing.png".into(),
            ..Project::default()
        };
        assert_eq!(
            resolve_image(&missing, Some(&path)),
            dunce::canonicalize(&shared).unwrap().join("missing.png")
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_as_through_symlinked_parent_keeps_relative_source_correct() {
        let directory = tempfile::tempdir().unwrap();
        let real_parent = directory.path().join("real/deep/projects");
        let assets = directory.path().join("assets");
        std::fs::create_dir_all(&real_parent).unwrap();
        std::fs::create_dir(&assets).unwrap();
        let image = assets.join("source.png");
        std::fs::write(&image, b"PNG").unwrap();
        let alias = directory.path().join("alias");
        std::os::unix::fs::symlink("real/deep/projects", &alias).unwrap();
        let project = Project {
            image_path: path_string(&dunce::canonicalize(&image).unwrap()).unwrap(),
            ..Project::default()
        };
        let destination = alias.join("saved.gbcs");
        save(&project, &destination).unwrap();
        assert!(
            std::fs::read_to_string(&destination)
                .unwrap()
                .contains("image,../../../assets/source.png")
        );
        assert_eq!(load(&destination).unwrap(), project);
        assert_eq!(load(&real_parent.join("saved.gbcs")).unwrap(), project);
    }
}
