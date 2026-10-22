use crate::value::ScratchValue;
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

pub const DEFAULT_MAX_PROJECT_MIB: u64 = 256;
const MIB: u64 = 1024 * 1024;
const MAX_ASSET: u64 = 32 * 1024 * 1024;
const MAX_ASSETS: u64 = 256 * 1024 * 1024;
#[derive(Debug, Deserialize)]
pub struct Project {
    pub targets: Vec<Target>,
    #[serde(default)]
    pub monitors: Vec<Monitor>,
    #[serde(skip)]
    pub assets: BTreeMap<String, Vec<u8>>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Monitor {
    pub id: String,
    #[serde(default)]
    pub mode: String,
    pub opcode: String,
    #[serde(default)]
    pub sprite_name: Option<String>,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub visible: bool,
    #[serde(default)]
    pub slider_min: f64,
    #[serde(default = "size")]
    pub slider_max: f64,
    #[serde(default = "visible")]
    pub is_discrete: bool,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub name: String,
    #[serde(default)]
    pub is_stage: bool,
    #[serde(default)]
    pub variables: BTreeMap<String, (String, ScratchValue)>,
    #[serde(default)]
    pub lists: BTreeMap<String, (String, Vec<ScratchValue>)>,
    #[serde(default, deserialize_with = "deserialize_blocks")]
    pub blocks: BTreeMap<String, Block>,
    #[serde(default)]
    pub costumes: Vec<Costume>,
    #[serde(default)]
    pub sounds: Vec<Sound>,
    #[serde(default = "size")]
    pub volume: f64,
    #[serde(default)]
    pub current_costume: usize,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default = "direction")]
    pub direction: f64,
    #[serde(default = "size")]
    pub size: f64,
    #[serde(default = "visible")]
    pub visible: bool,
    #[serde(default)]
    pub layer_order: i32,
    #[serde(default = "rotation")]
    pub rotation_style: String,
}
fn direction() -> f64 {
    90.0
}
fn size() -> f64 {
    100.0
}
fn visible() -> bool {
    true
}
fn rotation() -> String {
    "all around".into()
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Costume {
    pub name: String,
    pub md5ext: String,
    #[serde(default)]
    pub rotation_center_x: f64,
    #[serde(default)]
    pub rotation_center_y: f64,
    #[serde(default = "bitmap_resolution")]
    pub bitmap_resolution: f64,
}
#[derive(Debug, Deserialize)]
pub struct Sound {
    pub name: String,
    pub md5ext: String,
}
fn bitmap_resolution() -> f64 {
    1.0
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub opcode: String,
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub inputs: BTreeMap<String, Value>,
    #[serde(default)]
    pub fields: BTreeMap<String, Value>,
    #[serde(default)]
    pub mutation: Value,
    #[serde(default)]
    pub top_level: bool,
}
impl Block {
    pub fn field(&self, name: &str) -> Result<&str> {
        self.fields
            .get(name)
            .and_then(|v| v.get(0))
            .and_then(Value::as_str)
            .with_context(|| format!("{}: missing field {name}", self.opcode))
    }
}
fn deserialize_blocks<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, Block>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum SerializedBlock {
        Object(Block),
        Primitive(Vec<Value>),
    }
    let blocks = BTreeMap::<String, SerializedBlock>::deserialize(deserializer)?;
    blocks
        .into_iter()
        .map(|(id, block)| {
            let block = match block {
                SerializedBlock::Object(block) => block,
                SerializedBlock::Primitive(values) => {
                    // SB3 retains detached variable/list reporters as compact arrays.
                    // Normalize them so input references also use the usual lowering.
                    let (opcode, field) = match values.first().and_then(Value::as_u64) {
                        Some(12) => ("data_variable", "VARIABLE"),
                        Some(13) => ("data_listcontents", "LIST"),
                        _ => {
                            return Err(serde::de::Error::custom(format!(
                                "block {id}: unsupported compact reporter"
                            )));
                        }
                    };
                    let name = values.get(1).and_then(Value::as_str);
                    let variable = values.get(2).and_then(Value::as_str);
                    let (Some(name), Some(variable)) = (name, variable) else {
                        return Err(serde::de::Error::custom(format!(
                            "block {id}: invalid compact reporter"
                        )));
                    };
                    Block {
                        opcode: opcode.into(),
                        next: None,
                        inputs: BTreeMap::new(),
                        fields: BTreeMap::from([(
                            field.into(),
                            serde_json::json!([name, variable]),
                        )]),
                        mutation: Value::Null,
                        top_level: false,
                    }
                }
            };
            Ok((id, block))
        })
        .collect()
}
impl Project {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_max_json_bytes(path, DEFAULT_MAX_PROJECT_MIB * MIB)
    }
    /// Bound the decompressed JSON independently of ZIP size and costume assets.
    pub fn load_with_max_json_bytes(path: impl AsRef<Path>, max_json_bytes: u64) -> Result<Self> {
        ensure!(
            max_json_bytes > 0 && max_json_bytes < usize::MAX as u64,
            "project JSON size limit must fit in memory and be greater than zero"
        );
        let path = path.as_ref();
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        if path.extension().is_some_and(|e| e == "json") {
            return read_project_json(file, max_json_bytes);
        }
        let mut archive = zip::ZipArchive::new(file).context("open .sb3 ZIP archive")?;
        let mut project = read_project_json(
            archive
                .by_name("project.json")
                .context(".sb3 has no project.json")?,
            max_json_bytes,
        )?;
        let mut total = 0;
        for target in &project.targets {
            for asset in target
                .costumes
                .iter()
                .map(|c| &c.md5ext)
                .chain(target.sounds.iter().map(|s| &s.md5ext))
            {
                if project.assets.contains_key(asset) {
                    continue;
                }
                let mut data = Vec::new();
                archive
                    .by_name(asset)
                    .with_context(|| format!("missing asset {asset}"))?
                    .take(MAX_ASSET + 1)
                    .read_to_end(&mut data)?;
                total += data.len() as u64;
                ensure!(
                    data.len() as u64 <= MAX_ASSET && total <= MAX_ASSETS,
                    "project assets exceed size limit"
                );
                project.assets.insert(asset.clone(), data);
            }
        }
        Ok(project)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.targets.is_empty(), "project contains no targets");
        ensure!(
            self.targets.iter().filter(|t| t.is_stage).count() == 1,
            "project must contain exactly one stage"
        );
        for target in &self.targets {
            for (id, block) in &target.blocks {
                if let Some(next) = &block.next {
                    ensure!(
                        target.blocks.contains_key(next),
                        "{} block {id}: missing next block {next}",
                        target.name
                    );
                }
            }
            for costume in &target.costumes {
                if !costume.bitmap_resolution.is_finite() || costume.bitmap_resolution <= 0.0 {
                    bail!("{}: invalid costume bitmap resolution", target.name);
                }
            }
        }
        Ok(())
    }
}

fn read_project_json(reader: impl Read, limit: u64) -> Result<Project> {
    let mut json = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut json)
        .context("read project JSON")?;
    ensure!(
        json.len() as u64 <= limit,
        "project JSON exceeds size limit ({} bytes / {:.2} MiB); increase --max-project-mib",
        limit,
        limit as f64 / MIB as f64
    );
    let project: Project = serde_json::from_slice(&json).context("parse project JSON")?;
    project.validate()?;
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    #[test]
    fn large_plain_and_compressed_projects_use_the_new_default_limit() -> Result<()> {
        let mut json = br#"{"targets":[{"name":"Stage","isStage":true}]}"#.to_vec();
        json.resize(16 * MIB as usize + 1, b' ');
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let prefix =
            std::env::temp_dir().join(format!("msbjit-loader-{}-{nonce}", std::process::id()));
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        for extension in ["json", "sb3"] {
            let fixture = Fixture(prefix.with_extension(extension));
            let file = File::create_new(&fixture.0)?;
            if extension == "json" {
                let mut file = file;
                file.write_all(&json)?;
            } else {
                let mut archive = zip::ZipWriter::new(file);
                archive.start_file(
                    "project.json",
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )?;
                archive.write_all(&json)?;
                archive.finish()?;
            }
            assert_eq!(Project::load(&fixture.0)?.targets[0].name, "Stage");
            let error = Project::load_with_max_json_bytes(&fixture.0, 16 * MIB).unwrap_err();
            assert!(error.to_string().contains("--max-project-mib"), "{error}");
        }
        Ok(())
    }

    #[test]
    fn size_boundary_is_exact_and_reads_are_bounded_before_utf8_parsing() -> Result<()> {
        let json = br#"{"targets":[{"name":"Stage","isStage":true}]}"#;
        assert!(read_project_json(&json[..], json.len() as u64).is_ok());
        assert!(read_project_json(&json[..], json.len() as u64 - 1).is_err());
        let mut reader = Cursor::new(vec![0xff; 1024]);
        let error = read_project_json(&mut reader, 64).unwrap_err();
        assert!(error.to_string().contains("size limit"), "{error}");
        assert_eq!(reader.position(), 65);
        Ok(())
    }

    #[test]
    fn invalid_public_limits_are_rejected_before_opening_files() {
        for limit in [0, u64::MAX] {
            let error = Project::load_with_max_json_bytes("unused.sb3", limit).unwrap_err();
            assert!(error.to_string().contains("size limit"), "{error}");
        }
    }

    #[test]
    #[ignore = "requires MSBJIT_LARGE_PROJECT pointing to an external large project"]
    fn external_large_project_loads() -> Result<()> {
        let Ok(path) = std::env::var("MSBJIT_LARGE_PROJECT") else {
            eprintln!("Skipping external project fixture: set MSBJIT_LARGE_PROJECT");
            return Ok(());
        };
        let project = Project::load(path)?;
        assert!(!project.targets.is_empty());
        eprintln!("Loaded {} targets", project.targets.len());
        Ok(())
    }
}
