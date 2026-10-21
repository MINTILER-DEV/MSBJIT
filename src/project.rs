use crate::value::ScratchValue;
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

const MAX_JSON: u64 = 16 * 1024 * 1024;
const MAX_ASSET: u64 = 32 * 1024 * 1024;
const MAX_ASSETS: u64 = 256 * 1024 * 1024;
#[derive(Debug, Deserialize)]
pub struct Project {
    pub targets: Vec<Target>,
    #[serde(skip)]
    pub assets: BTreeMap<String, Vec<u8>>,
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
    pub blocks: BTreeMap<String, Block>,
    #[serde(default)]
    pub costumes: Vec<Costume>,
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
impl Project {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        if path.extension().is_some_and(|e| e == "json") {
            let mut json = String::new();
            file.take(MAX_JSON + 1).read_to_string(&mut json)?;
            ensure!(json.len() as u64 <= MAX_JSON, "project JSON exceeds 16 MiB");
            let project: Self = serde_json::from_str(&json).context("parse project JSON")?;
            project.validate()?;
            return Ok(project);
        }
        let mut archive = zip::ZipArchive::new(file).context("open .sb3 ZIP archive")?;
        let mut json = String::new();
        archive
            .by_name("project.json")
            .context(".sb3 has no project.json")?
            .take(MAX_JSON + 1)
            .read_to_string(&mut json)?;
        ensure!(json.len() as u64 <= MAX_JSON, "project JSON exceeds 16 MiB");
        let mut project: Self = serde_json::from_str(&json).context("parse project.json")?;
        project.validate()?;
        let mut total = 0;
        for target in &project.targets {
            for costume in &target.costumes {
                if project.assets.contains_key(&costume.md5ext) {
                    continue;
                }
                let mut data = Vec::new();
                archive
                    .by_name(&costume.md5ext)
                    .with_context(|| format!("missing costume {}", costume.md5ext))?
                    .take(MAX_ASSET + 1)
                    .read_to_end(&mut data)?;
                total += data.len() as u64;
                ensure!(
                    data.len() as u64 <= MAX_ASSET && total <= MAX_ASSETS,
                    "costume assets exceed size limit"
                );
                project.assets.insert(costume.md5ext.clone(), data);
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
