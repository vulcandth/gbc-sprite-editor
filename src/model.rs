//! The editable project model, independent of the GUI and image exporter.

use std::collections::HashSet;

pub const MAX_CELS: usize = 4096;
pub const MAX_SLICES: usize = 65_536;
pub const MAX_COORDINATE: i32 = 1_000_000;
pub const MAX_SLICE_DIMENSION: u32 = 256;
pub const MAX_NAME_CHARACTERS: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    Dmg,
    #[default]
    Cgb,
}

impl Target {
    /// The largest valid OBJ palette number on the selected hardware.
    pub fn max_palette(self) -> u8 {
        match self {
            Self::Dmg => 1,
            Self::Cgb => 7,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    /// Absolute after loading a project. Relative paths in a new project use cwd.
    pub image_path: String,
    pub target: Target,
    /// A source RGB color that should become transparent, in addition to alpha.
    pub transparent_color: Option<[u8; 3]>,
    pub cels: Vec<Cel>,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            image_path: String::new(),
            target: Target::Cgb,
            transparent_color: None,
            cels: vec![Cel::default()],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cel {
    pub name: String,
    /// The cel origin in source-image pixel coordinates.
    pub origin_x: i32,
    pub origin_y: i32,
    pub slices: Vec<Slice>,
}

impl Default for Cel {
    fn default() -> Self {
        Self {
            name: "cel_0".into(),
            origin_x: 0,
            origin_y: 0,
            slices: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice {
    pub name: String,
    /// Top-left in source-image pixels, independent of the cel origin.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub palette: u8,
    /// None includes every color; Some(empty) includes no colors.
    pub colors: Option<Vec<[u8; 3]>>,
}

impl Default for Slice {
    fn default() -> Self {
        Self {
            name: "slice_0".into(),
            x: 0,
            y: 0,
            width: 8,
            height: 16,
            palette: 0,
            colors: None,
        }
    }
}

impl Slice {
    /// Half-open rectangle hit test. Safe even before validating imported data.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        let dx = i64::from(x) - i64::from(self.x);
        let dy = i64::from(y) - i64::from(self.y);
        dx >= 0 && dy >= 0 && dx < i64::from(self.width) && dy < i64::from(self.height)
    }
}

impl Project {
    /// Validate structural limits. Hardware tile/color warnings belong to export,
    /// because arbitrary slice sizes and overlapping palette layers are editable.
    pub fn validate(&self) -> Result<(), String> {
        if self.image_path.contains('\0') {
            return Err("Image path contains a NUL character".into());
        }
        if self.cels.is_empty() || self.cels.len() > MAX_CELS {
            return Err(format!("A project must contain 1..={MAX_CELS} cels"));
        }
        let mut slice_count = 0usize;
        for (cel_index, cel) in self.cels.iter().enumerate() {
            validate_name(&cel.name, &format!("Cel {cel_index}"))?;
            let label = format!("Cel {cel_index} ({:?})", cel.name);
            validate_position(cel.origin_x, cel.origin_y, &format!("{label} origin"))?;
            slice_count = slice_count
                .checked_add(cel.slices.len())
                .ok_or_else(|| "Too many slices".to_string())?;
            if slice_count > MAX_SLICES {
                return Err(format!("A project may contain at most {MAX_SLICES} slices"));
            }
            for (slice_index, slice) in cel.slices.iter().enumerate() {
                validate_name(
                    &slice.name,
                    &format!("Cel {cel_index}, slice {slice_index}"),
                )?;
                let label = format!("{label}, slice {slice_index} ({:?})", slice.name);
                validate_position(slice.x, slice.y, &label)?;
                if !(1..=MAX_SLICE_DIMENSION).contains(&slice.width)
                    || !(1..=MAX_SLICE_DIMENSION).contains(&slice.height)
                {
                    return Err(format!(
                        "{label}: width and height must be 1..={MAX_SLICE_DIMENSION} pixels"
                    ));
                }
                if slice.palette > self.target.max_palette() {
                    return Err(format!(
                        "{label}: palette must be 0..={} for {:?}",
                        self.target.max_palette(),
                        self.target
                    ));
                }
                if let Some(colors) = &slice.colors {
                    if colors.len() > 256 {
                        return Err(format!(
                            "{label}: color mask may contain at most 256 colors"
                        ));
                    }
                    let unique: HashSet<_> = colors.iter().collect();
                    if unique.len() != colors.len() {
                        return Err(format!("{label}: color mask contains duplicate colors"));
                    }
                }
            }
        }
        Ok(())
    }
}

fn validate_name(name: &str, label: &str) -> Result<(), String> {
    // Bound diagnostic and export sizes before interpolating untrusted names.
    if name.chars().take(MAX_NAME_CHARACTERS + 1).count() > MAX_NAME_CHARACTERS {
        return Err(format!(
            "{label}: name may contain at most {MAX_NAME_CHARACTERS} characters"
        ));
    }
    if name.contains('\0') {
        return Err(format!("{label}: name contains a NUL character"));
    }
    Ok(())
}

fn validate_position(x: i32, y: i32, label: &str) -> Result<(), String> {
    if !(-MAX_COORDINATE..=MAX_COORDINATE).contains(&x)
        || !(-MAX_COORDINATE..=MAX_COORDINATE).contains(&y)
    {
        Err(format!(
            "{label}: coordinates must be within +/-{MAX_COORDINATE} pixels"
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_slice(slice: Slice) -> Project {
        let mut project = Project::default();
        project.cels[0].slices.push(slice);
        project
    }

    #[test]
    fn default_is_valid() {
        assert_eq!(Project::default().validate(), Ok(()));
        assert_eq!(Slice::default().height, 16);
    }

    #[test]
    fn names_are_bounded_by_unicode_characters_and_errors_stay_small() {
        let mut project = with_slice(Slice {
            name: "🦊".repeat(MAX_NAME_CHARACTERS),
            ..Slice::default()
        });
        project.cels[0].name = "猫".repeat(MAX_NAME_CHARACTERS);
        assert!(project.validate().is_ok());
        project.cels[0].name.push('猫');
        let error = project.validate().unwrap_err();
        assert!(error.contains("at most 256 characters"));
        assert!(error.len() < 100);
        project.cels[0].name = "cel".into();
        project.cels[0].slices[0].name.push('🦊');
        let error = project.validate().unwrap_err();
        assert!(error.contains("slice 0"));
        assert!(error.len() < 100);
    }

    #[test]
    fn rectangle_edges_and_extreme_coordinates_do_not_overflow() {
        let slice = Slice {
            x: -4,
            y: -8,
            ..Slice::default()
        };
        assert!(slice.contains(-4, -8));
        assert!(slice.contains(3, 7));
        assert!(!slice.contains(4, 7));
        assert!(!slice.contains(3, 8));
        assert!(!slice.contains(-5, -8));
        let huge = Slice {
            x: i32::MIN,
            width: u32::MAX,
            ..Slice::default()
        };
        assert!(!huge.contains(i32::MAX, 0));
        assert!(huge.contains(i32::MAX - 1, 0));
    }

    #[test]
    fn reject_invalid_dimensions_and_coordinates() {
        for (width, height) in [(0, 8), (8, 0), (257, 8), (8, u32::MAX)] {
            assert!(
                with_slice(Slice {
                    width,
                    height,
                    ..Slice::default()
                })
                .validate()
                .is_err()
            );
        }
        assert!(
            with_slice(Slice {
                x: i32::MIN,
                ..Slice::default()
            })
            .validate()
            .is_err()
        );
        let mut project = Project::default();
        project.cels[0].origin_y = MAX_COORDINATE + 1;
        assert!(project.validate().is_err());
    }

    #[test]
    fn palette_limits_depend_on_target() {
        let mut project = with_slice(Slice {
            palette: 7,
            ..Slice::default()
        });
        assert!(project.validate().is_ok());
        project.target = Target::Dmg;
        assert!(project.validate().is_err());
        project.cels[0].slices[0].palette = 1;
        assert!(project.validate().is_ok());
        project.target = Target::Cgb;
        project.cels[0].slices[0].palette = 8;
        assert!(project.validate().is_err());
    }

    #[test]
    fn color_mask_distinguishes_empty_and_rejects_duplicates() {
        assert!(
            with_slice(Slice {
                colors: Some(vec![]),
                ..Slice::default()
            })
            .validate()
            .is_ok()
        );
        assert!(
            with_slice(Slice {
                colors: Some(vec![[1, 2, 3], [1, 2, 3]]),
                ..Slice::default()
            })
            .validate()
            .is_err()
        );
        let colors = (0..=256).map(|n| [(n / 256) as u8, n as u8, 0]).collect();
        assert!(
            with_slice(Slice {
                colors: Some(colors),
                ..Slice::default()
            })
            .validate()
            .is_err()
        );
    }
}
