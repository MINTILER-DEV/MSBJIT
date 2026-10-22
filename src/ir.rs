//! Validated Scratch blocks lowered to a control-flow graph. No opcode dispatch
//! or AST walking is performed while native code is running.
use crate::{
    project::{Block, Project},
    value::{ScratchValue, Values},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Debug, Clone)]
pub(crate) enum Expr {
    Constant(u64),
    Variable(usize),
    Argument(usize),
    Binary(u32, Box<Expr>, Box<Expr>),
    Unary(u32, Box<Expr>),
    State(u32),
    List(u32, usize, Box<Expr>),
}
#[derive(Debug, Clone)]
pub(crate) enum Op {
    Set(usize, Expr),
    Change(usize, Expr),
    Action(u32, Expr, Expr),
    List(u32, usize, Expr, Expr),
    Branch(Expr, usize, usize),
    Jump(usize, bool),
    RepeatInit(usize, Expr, usize),
    RepeatCheck(usize, usize, usize),
    RepeatDec(usize, usize),
    ForEachCheck(usize, usize, Expr, usize, usize),
    Wait(Expr),
    Yield(usize),
    Broadcast(Expr, bool),
    Call(usize, Vec<Expr>),
    Return,
    Stop(u32),
}
#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub op: Op,
    pub next: usize,
}
#[derive(Debug)]
pub(crate) enum Hat {
    Flag,
    Broadcast(String),
    Key(String),
    Click,
}
#[derive(Debug)]
pub(crate) struct Script {
    pub target: usize,
    pub entry: usize,
    pub hat: Hat,
}
#[derive(Debug)]
pub(crate) struct Procedure {
    pub entry: usize,
    pub warp: bool,
    pub args: Vec<String>,
    pub names: Vec<String>,
    pub defaults: Vec<ScratchValue>,
}
pub(crate) struct Program {
    pub nodes: Vec<Node>,
    pub scripts: Vec<Script>,
    pub procedures: Vec<Procedure>,
    pub variable_names: Vec<(usize, String, String)>,
    pub initial_variables: Vec<u64>,
    pub list_names: Vec<(usize, String, String)>,
    pub initial_lists: Vec<Vec<u64>>,
    pub constants: Vec<u64>,
    pub values: Values,
    pub loop_slots: usize,
}
struct Lower<'a> {
    project: &'a Project,
    program: Program,
    vars: BTreeMap<(usize, String), usize>,
    lists: BTreeMap<(usize, String), usize>,
    procs: BTreeMap<(usize, String), usize>,
    target: usize,
    args: Vec<String>,
    active: HashSet<String>,
    depth: usize,
    pending_procedures: BTreeSet<usize>,
}
impl Program {
    pub fn lower(project: &Project) -> Result<Self> {
        project.validate()?;
        let mut l = Lower {
            project,
            program: Self {
                nodes: vec![Node {
                    op: Op::Return,
                    next: 0,
                }],
                scripts: vec![],
                procedures: vec![],
                variable_names: vec![],
                initial_variables: vec![],
                list_names: vec![],
                initial_lists: vec![],
                constants: vec![],
                values: Values::default(),
                loop_slots: 0,
            },
            vars: BTreeMap::new(),
            lists: BTreeMap::new(),
            procs: BTreeMap::new(),
            target: 0,
            args: vec![],
            active: HashSet::new(),
            depth: 0,
            pending_procedures: BTreeSet::new(),
        };
        for (target, t) in project.targets.iter().enumerate() {
            for (id, (name, items)) in &t.lists {
                let slot = l.program.initial_lists.len();
                l.lists.insert((target, id.clone()), slot);
                l.program
                    .list_names
                    .push((target, id.clone(), name.clone()));
                l.program.initial_lists.push(
                    items
                        .iter()
                        .map(|v| l.program.values.encode(v.clone()))
                        .collect(),
                );
            }
            for (id, (name, value)) in &t.variables {
                let slot = l.program.initial_variables.len();
                l.vars.insert((target, id.clone()), slot);
                l.program
                    .variable_names
                    .push((target, id.clone(), name.clone()));
                let value = l.program.values.encode(value.clone());
                l.program.initial_variables.push(value);
            }
        }
        // Declare all custom blocks before lowering bodies (including recursion).
        let mut definitions = vec![];
        for (target, t) in project.targets.iter().enumerate() {
            for (id, b) in &t.blocks {
                if b.opcode != "procedures_definition" {
                    continue;
                }
                l.target = target;
                let proto_id = l
                    .block_input(b, "custom_block")?
                    .context("missing procedure prototype")?;
                let proto = l.block(&proto_id)?;
                let code = proto.mutation["proccode"]
                    .as_str()
                    .context("missing procedure code")?
                    .to_owned();
                let args: Vec<String> = mutation_array(&proto.mutation, "argumentids")?;
                let names: Vec<String> = mutation_array(&proto.mutation, "argumentnames")?;
                let defaults: Vec<ScratchValue> =
                    mutation_array(&proto.mutation, "argumentdefaults")?;
                // Scratch indexes defaults only for declared parameter IDs.
                // Edited projects can retain defaults for deleted parameters.
                ensure!(
                    args.len() == names.len() && args.len() <= defaults.len(),
                    "procedure {code}: inconsistent arguments"
                );
                let warp = proto.mutation["warp"]
                    .as_bool()
                    .unwrap_or_else(|| proto.mutation["warp"] == "true");
                let entry = l.add(Op::Jump(0, false), 0)?;
                let proc_id = l.program.procedures.len();
                ensure!(
                    l.procs.insert((target, code.clone()), proc_id).is_none(),
                    "duplicate custom block {code}"
                );
                l.program.procedures.push(Procedure {
                    entry,
                    warp,
                    args,
                    names,
                    defaults,
                });
                definitions.push((target, id.clone(), proc_id));
            }
        }
        for (target, t) in project.targets.iter().enumerate() {
            l.target = target;
            for (id, b) in &t.blocks {
                if !b.top_level {
                    continue;
                }
                let hat = match b.opcode.as_str() {
                    "event_whenflagclicked" => Hat::Flag,
                    "event_whenkeypressed" => Hat::Key(b.field("KEY_OPTION")?.to_owned()),
                    "event_whenthisspriteclicked" | "event_whenstageclicked" => Hat::Click,
                    "event_whenbroadcastreceived" => {
                        Hat::Broadcast(b.field("BROADCAST_OPTION")?.to_owned())
                    }
                    "procedures_definition" => continue,
                    s if s.starts_with("event_when") || s.starts_with("control_start_as_clone") => {
                        bail!("{} block {id}: unsupported event {s}", t.name)
                    }
                    _ => continue, // Scratch keeps detached stacks in the workspace.
                };
                let entry = l
                    .chain(b.next.as_deref(), 0)
                    .with_context(|| format!("{} script {id}", t.name))?;
                l.program.scripts.push(Script { target, entry, hat });
            }
        }
        let mut compiled = HashSet::new();
        while let Some(proc_id) = l.pending_procedures.pop_first() {
            if !compiled.insert(proc_id) {
                continue;
            }
            let (target, id, _) = &definitions[proc_id];
            l.target = *target;
            l.args = l.program.procedures[proc_id].names.clone();
            let start = l.block(id)?.next.clone();
            let body = l.chain(start.as_deref(), 0)?;
            let entry = l.program.procedures[proc_id].entry;
            l.program.nodes[entry].op = Op::Jump(body, false);
        }
        // Only string handles are GC roots; constants never change at runtime.
        l.program
            .constants
            .retain(|v| v & 0xffff_0000_0000_0000 == crate::value::STRING_TAG);
        Ok(l.program)
    }
}
fn mutation_array<T: serde::de::DeserializeOwned>(v: &Value, key: &str) -> Result<Vec<T>> {
    let value = v
        .get(key)
        .with_context(|| format!("missing mutation {key}"))?;
    if let Some(s) = value.as_str() {
        Ok(serde_json::from_str(s)?)
    } else {
        Ok(serde_json::from_value(value.clone())?)
    }
}
impl Lower<'_> {
    fn block(&self, id: &str) -> Result<&Block> {
        self.project.targets[self.target]
            .blocks
            .get(id)
            .with_context(|| format!("missing block {id}"))
    }
    fn add(&mut self, op: Op, next: usize) -> Result<usize> {
        ensure!(
            self.program.nodes.len() < 20_000,
            "project exceeds 20,000 compiled instructions"
        );
        let id = self.program.nodes.len();
        self.program.nodes.push(Node { op, next });
        Ok(id)
    }
    fn constant(&mut self, value: ScratchValue) -> Expr {
        let value = self.program.values.encode(value);
        self.program.constants.push(value);
        Expr::Constant(value)
    }
    fn zero(&mut self) -> Expr {
        self.constant(ScratchValue::Number(0.0))
    }
    fn variable(&self, id: &str) -> Result<usize> {
        self.vars
            .get(&(self.target, id.to_owned()))
            .or_else(|| {
                let stage = self.project.targets.iter().position(|t| t.is_stage)?;
                self.vars.get(&(stage, id.to_owned()))
            })
            .copied()
            .with_context(|| format!("unknown variable {id}"))
    }
    fn var_field(&self, b: &Block) -> Result<usize> {
        let id = b
            .fields
            .get("VARIABLE")
            .and_then(|f| f.get(1))
            .and_then(Value::as_str)
            .context("missing variable ID")?;
        self.variable(id)
    }
    fn list_field(&self, b: &Block) -> Result<usize> {
        let id = b
            .fields
            .get("LIST")
            .and_then(|f| f.get(1))
            .and_then(Value::as_str)
            .context("missing list ID")?;
        self.list(id)
    }
    fn list(&self, id: &str) -> Result<usize> {
        self.lists
            .get(&(self.target, id.to_owned()))
            .or_else(|| {
                let stage = self.project.targets.iter().position(|t| t.is_stage)?;
                self.lists.get(&(stage, id.to_owned()))
            })
            .copied()
            .with_context(|| format!("unknown list {id}"))
    }
    fn input<'a>(&self, b: &'a Block, name: &str) -> Result<&'a Value> {
        let input = b
            .inputs
            .get(name)
            .with_context(|| format!("{}: missing input {name}", b.opcode))?;
        input
            .get(1)
            .filter(|v| !v.is_null())
            .or_else(|| input.get(2))
            .context("invalid input encoding")
    }
    fn block_input(&self, b: &Block, name: &str) -> Result<Option<String>> {
        let Some(input) = b.inputs.get(name) else {
            return Ok(None);
        };
        match input.get(1) {
            Some(Value::String(id)) => {
                self.block(id)?;
                Ok(Some(id.clone()))
            }
            Some(Value::Null) | None => Ok(None),
            _ => bail!("{}: {name} must reference a block", b.opcode),
        }
    }
    fn expr_input(&mut self, b: &Block, name: &str) -> Result<Expr> {
        self.expr(self.input(b, name)?, 0)
    }
    fn expr(&mut self, v: &Value, depth: usize) -> Result<Expr> {
        ensure!(
            depth < 128,
            "reporter graph exceeds depth limit or is cyclic"
        );
        if let Some(id) = v.as_str() {
            let b = self.block(id)?;
            let opcode = b.opcode.clone();
            if opcode == "data_variable" {
                return Ok(Expr::Variable(self.var_field(b)?));
            }
            let list_op = match opcode.as_str() {
                "data_itemoflist" => Some((5, Some("INDEX"))),
                "data_lengthoflist" => Some((6, None)),
                "data_listcontents" => Some((7, None)),
                "data_itemnumoflist" => Some((8, Some("ITEM"))),
                "data_listcontainsitem" => Some((9, Some("ITEM"))),
                _ => None,
            };
            if let Some((op, input)) = list_op {
                let slot = self.list_field(b)?;
                let arg = if let Some(input) = input {
                    self.expr(&self.input(b, input)?.clone(), depth + 1)?
                } else {
                    self.zero()
                };
                return Ok(Expr::List(op, slot, Box::new(arg)));
            }
            if opcode.starts_with("argument_reporter_") {
                let name = b.field("VALUE")?;
                return Ok(self
                    .args
                    .iter()
                    .position(|a| a == name)
                    .map(Expr::Argument)
                    .unwrap_or_else(|| self.zero()));
            }
            let binary = match opcode.as_str() {
                "operator_add" => Some((0, "NUM1", "NUM2")),
                "operator_subtract" => Some((1, "NUM1", "NUM2")),
                "operator_multiply" => Some((2, "NUM1", "NUM2")),
                "operator_divide" => Some((3, "NUM1", "NUM2")),
                "operator_mod" => Some((4, "NUM1", "NUM2")),
                "operator_lt" => Some((5, "OPERAND1", "OPERAND2")),
                "operator_equals" => Some((6, "OPERAND1", "OPERAND2")),
                "operator_gt" => Some((7, "OPERAND1", "OPERAND2")),
                "operator_and" => Some((8, "OPERAND1", "OPERAND2")),
                "operator_or" => Some((9, "OPERAND1", "OPERAND2")),
                "operator_join" => Some((10, "STRING1", "STRING2")),
                "operator_contains" => Some((11, "STRING1", "STRING2")),
                "operator_letter_of" => Some((12, "LETTER", "STRING")),
                "operator_random" => Some((13, "FROM", "TO")),
                _ => None,
            };
            if let Some((op, a, c)) = binary {
                let a = self.input(b, a)?.clone();
                let c = self.input(b, c)?.clone();
                return Ok(Expr::Binary(
                    op,
                    Box::new(self.expr(&a, depth + 1)?),
                    Box::new(self.expr(&c, depth + 1)?),
                ));
            }
            let unary = match opcode.as_str() {
                "sensing_touchingobject" => Some((18, "TOUCHINGOBJECTMENU")),
                "operator_not" => Some((0, "OPERAND")),
                "operator_length" => Some((1, "STRING")),
                "operator_round" => Some((2, "NUM")),
                "sensing_keypressed" => Some((17, "KEY_OPTION")),
                "operator_mathop" => {
                    let op = match b.field("OPERATOR")? {
                        "abs" => 3,
                        "floor" => 4,
                        "ceiling" => 5,
                        "sqrt" => 6,
                        "sin" => 7,
                        "cos" => 8,
                        "tan" => 9,
                        "asin" => 10,
                        "acos" => 11,
                        "atan" => 12,
                        "ln" => 13,
                        "log" => 14,
                        "e ^" => 15,
                        "10 ^" => 16,
                        s => bail!("unsupported math operator {s}"),
                    };
                    Some((op, "NUM"))
                }
                _ => None,
            };
            if let Some((op, input)) = unary {
                let a = self.input(b, input)?.clone();
                return Ok(Expr::Unary(op, Box::new(self.expr(&a, depth + 1)?)));
            }
            let state = match opcode.as_str() {
                "motion_xposition" => 0,
                "motion_yposition" => 1,
                "motion_direction" => 2,
                "looks_size" => 3,
                "sensing_timer" => 4,
                "sensing_mousex" => 7,
                "sensing_mousey" => 8,
                "sensing_mousedown" => 9,
                "sensing_dayssince2000" => 10,
                "sensing_touchingobjectmenu" => {
                    return Ok(
                        self.constant(ScratchValue::String(b.field("TOUCHINGOBJECTMENU")?.into()))
                    );
                }
                "sound_sounds_menu" => {
                    return Ok(self.constant(ScratchValue::String(b.field("SOUND_MENU")?.into())));
                }
                "sensing_keyoptions" => {
                    return Ok(self.constant(ScratchValue::String(b.field("KEY_OPTION")?.into())));
                }
                "pen_menu_colorParam" => {
                    return Ok(self.constant(ScratchValue::String(b.field("colorParam")?.into())));
                }
                "looks_costumenumbername" => {
                    if b.field("NUMBER_NAME")? == "number" {
                        5
                    } else {
                        6
                    }
                }
                "event_broadcast_menu" => {
                    return Ok(
                        self.constant(ScratchValue::String(b.field("BROADCAST_OPTION")?.into()))
                    );
                }
                "looks_costume" => {
                    return Ok(self.constant(ScratchValue::String(b.field("COSTUME")?.into())));
                }
                s => bail!("unsupported reporter {s} ({id})"),
            };
            return Ok(Expr::State(state));
        }
        let array = v.as_array().context("invalid Scratch input literal")?;
        let kind = array
            .first()
            .and_then(Value::as_u64)
            .context("missing literal type")?;
        if kind == 12 {
            let id = array
                .get(2)
                .and_then(Value::as_str)
                .context("missing variable literal ID")?;
            return Ok(Expr::Variable(self.variable(id)?));
        }
        if kind == 13 {
            let id = array
                .get(2)
                .and_then(Value::as_str)
                .context("missing list literal ID")?;
            return Ok(Expr::List(7, self.list(id)?, Box::new(self.zero())));
        }
        ensure!((4..=11).contains(&kind), "unsupported literal type {kind}");
        let raw = array.get(1).context("missing literal value")?;
        let value = if kind <= 8 {
            let numeric: ScratchValue = serde_json::from_value(raw.clone())?;
            ScratchValue::Number(numeric.to_number())
        } else {
            ScratchValue::String(raw.as_str().context("invalid string literal")?.into())
        };
        Ok(self.constant(value))
    }
    fn chain(&mut self, start: Option<&str>, continuation: usize) -> Result<usize> {
        ensure!(
            self.depth < 128,
            "control graph exceeds depth limit or is cyclic"
        );
        self.depth += 1;
        let mut ids = vec![];
        let mut current = start.map(str::to_owned);
        while let Some(id) = current {
            ensure!(self.active.insert(id.clone()), "cyclic block graph at {id}");
            ensure!(ids.len() < 20_000, "block chain exceeds limit");
            current = self.block(&id)?.next.clone();
            ids.push(id);
        }
        let mut next = continuation;
        for id in ids.iter().rev() {
            next = self
                .statement(id, next)
                .with_context(|| format!("block {id}"))?;
        }
        for id in ids {
            self.active.remove(&id);
        }
        self.depth -= 1;
        Ok(next)
    }
    fn statement(&mut self, id: &str, next: usize) -> Result<usize> {
        // Borrow blocks from a separate reference to permit recursive lowering.
        let b = &self.project.targets[self.target].blocks[id];
        let op = match b.opcode.as_str() {
            "data_setvariableto" => Op::Set(self.var_field(b)?, self.expr_input(b, "VALUE")?),
            "data_changevariableby" => Op::Change(self.var_field(b)?, self.expr_input(b, "VALUE")?),
            "data_addtolist" => Op::List(
                0,
                self.list_field(b)?,
                self.expr_input(b, "ITEM")?,
                self.zero(),
            ),
            "data_deleteoflist" => Op::List(
                1,
                self.list_field(b)?,
                self.expr_input(b, "INDEX")?,
                self.zero(),
            ),
            "data_deletealloflist" => Op::List(2, self.list_field(b)?, self.zero(), self.zero()),
            "data_insertatlist" => Op::List(
                3,
                self.list_field(b)?,
                self.expr_input(b, "INDEX")?,
                self.expr_input(b, "ITEM")?,
            ),
            "data_replaceitemoflist" => Op::List(
                4,
                self.list_field(b)?,
                self.expr_input(b, "INDEX")?,
                self.expr_input(b, "ITEM")?,
            ),
            "control_repeat" => {
                let slot = self.program.loop_slots;
                self.program.loop_slots += 1;
                let check = self.add(Op::Return, 0)?;
                let dec = self.add(Op::RepeatDec(slot, check), check)?;
                let body = self.chain(self.block_input(b, "SUBSTACK")?.as_deref(), dec)?;
                self.program.nodes[check].op = Op::RepeatCheck(slot, body, next);
                Op::RepeatInit(slot, self.expr_input(b, "TIMES")?, check)
            }
            "control_forever" => {
                let back = self.add(Op::Return, 0)?;
                let body = self.chain(self.block_input(b, "SUBSTACK")?.as_deref(), back)?;
                self.program.nodes[back].op = Op::Jump(body, true);
                Op::Jump(body, false)
            }
            "control_for_each" => {
                let slot = self.program.loop_slots;
                self.program.loop_slots += 1;
                let check = self.add(Op::Return, 0)?;
                let back = self.add(Op::Jump(check, true), check)?;
                let body = self.chain(self.block_input(b, "SUBSTACK")?.as_deref(), back)?;
                self.program.nodes[check].op = Op::ForEachCheck(
                    slot,
                    self.var_field(b)?,
                    self.expr_input(b, "VALUE")?,
                    body,
                    next,
                );
                Op::RepeatInit(slot, self.zero(), check)
            }
            "control_repeat_until" | "control_wait_until" => {
                let check = self.add(Op::Return, 0)?;
                let back = self.add(
                    if b.opcode == "control_wait_until" {
                        Op::Yield(check)
                    } else {
                        Op::Jump(check, true)
                    },
                    check,
                )?;
                let body = if b.opcode == "control_wait_until" {
                    back
                } else {
                    self.chain(self.block_input(b, "SUBSTACK")?.as_deref(), back)?
                };
                self.program.nodes[check].op =
                    Op::Branch(self.expr_input(b, "CONDITION")?, next, body);
                Op::Jump(check, false)
            }
            "control_while" => {
                let check = self.add(Op::Return, 0)?;
                let back = self.add(Op::Jump(check, true), check)?;
                let body = self.chain(self.block_input(b, "SUBSTACK")?.as_deref(), back)?;
                self.program.nodes[check].op =
                    Op::Branch(self.expr_input(b, "CONDITION")?, body, next);
                Op::Jump(check, false)
            }
            "control_if" | "control_if_else" => {
                let yes = self.chain(self.block_input(b, "SUBSTACK")?.as_deref(), next)?;
                let no = if b.opcode == "control_if_else" {
                    self.chain(self.block_input(b, "SUBSTACK2")?.as_deref(), next)?
                } else {
                    next
                };
                Op::Branch(self.expr_input(b, "CONDITION")?, yes, no)
            }
            "control_wait" => Op::Wait(self.expr_input(b, "DURATION")?),
            "event_broadcast" | "event_broadcastandwait" => Op::Broadcast(
                self.expr_input(b, "BROADCAST_INPUT")?,
                b.opcode == "event_broadcastandwait",
            ),
            "procedures_call" => {
                let code = b.mutation["proccode"]
                    .as_str()
                    .context("missing call procedure code")?;
                let Some(&proc_id) = self.procs.get(&(self.target, code.into())) else {
                    // Scratch treats a call whose definition was removed as a
                    // no-op, including when a stack was copied between sprites.
                    return Ok(next);
                };
                self.pending_procedures.insert(proc_id);
                let params = self.program.procedures[proc_id].args.clone();
                let defaults = self.program.procedures[proc_id].defaults.clone();
                let mut args = vec![];
                for (i, param) in params.iter().enumerate() {
                    args.push(if b.inputs.contains_key(param) {
                        self.expr_input(b, param)?
                    } else {
                        self.constant(defaults[i].clone())
                    });
                }
                Op::Call(proc_id, args)
            }
            "control_stop" => Op::Stop(match b.field("STOP_OPTION")? {
                "all" => 0,
                "this script" => 1,
                "other scripts in sprite" => 2,
                s => bail!("unknown stop option {s}"),
            }),
            "motion_movesteps" => Op::Action(0, self.expr_input(b, "STEPS")?, self.zero()),
            "motion_gotoxy" => Op::Action(1, self.expr_input(b, "X")?, self.expr_input(b, "Y")?),
            "motion_setx" => Op::Action(2, self.expr_input(b, "X")?, self.zero()),
            "motion_sety" => Op::Action(3, self.expr_input(b, "Y")?, self.zero()),
            "motion_changexby" => Op::Action(4, self.expr_input(b, "DX")?, self.zero()),
            "motion_changeyby" => Op::Action(5, self.expr_input(b, "DY")?, self.zero()),
            "motion_pointindirection" => {
                Op::Action(6, self.expr_input(b, "DIRECTION")?, self.zero())
            }
            "motion_turnright" => Op::Action(7, self.expr_input(b, "DEGREES")?, self.zero()),
            "motion_turnleft" => Op::Action(8, self.expr_input(b, "DEGREES")?, self.zero()),
            "looks_show" => Op::Action(9, self.zero(), self.zero()),
            "looks_hide" => Op::Action(10, self.zero(), self.zero()),
            "looks_setsizeto" => Op::Action(11, self.expr_input(b, "SIZE")?, self.zero()),
            "looks_changesizeby" => Op::Action(12, self.expr_input(b, "CHANGE")?, self.zero()),
            "looks_say" | "looks_think" => {
                Op::Action(13, self.expr_input(b, "MESSAGE")?, self.zero())
            }
            "looks_sayforsecs" | "looks_thinkforsecs" => {
                let seconds = self.expr_input(b, "SECS")?;
                let wait = self.add(Op::Wait(seconds), next)?;
                let message = self.expr_input(b, "MESSAGE")?;
                let zero = self.zero();
                return self.add(Op::Action(13, message, zero), wait);
            }
            "looks_switchcostumeto" => Op::Action(14, self.expr_input(b, "COSTUME")?, self.zero()),
            "looks_nextcostume" => Op::Action(15, self.zero(), self.zero()),
            "sensing_resettimer" => Op::Action(16, self.zero(), self.zero()),
            "motion_setrotationstyle" => {
                let s = self.constant(ScratchValue::String(b.field("STYLE")?.into()));
                Op::Action(17, s, self.zero())
            }
            "pen_clear" => Op::Action(18, self.zero(), self.zero()),
            "pen_penDown" => Op::Action(19, self.zero(), self.zero()),
            "pen_penUp" => Op::Action(20, self.zero(), self.zero()),
            "pen_setPenSizeTo" => Op::Action(21, self.expr_input(b, "SIZE")?, self.zero()),
            "pen_setPenColorParamTo" => Op::Action(
                22,
                self.expr_input(b, "COLOR_PARAM")?,
                self.expr_input(b, "VALUE")?,
            ),
            "pen_changePenColorParamBy" => Op::Action(
                23,
                self.expr_input(b, "COLOR_PARAM")?,
                self.expr_input(b, "VALUE")?,
            ),
            "pen_setPenColorToColor" => Op::Action(24, self.expr_input(b, "COLOR")?, self.zero()),
            "pen_changePenSizeBy" => Op::Action(25, self.expr_input(b, "SIZE")?, self.zero()),
            "pen_stamp" => Op::Action(26, self.zero(), self.zero()),
            "sound_play" => Op::Action(27, self.expr_input(b, "SOUND_MENU")?, self.zero()),
            s => bail!("unsupported command {s}"),
        };
        self.add(op, next)
    }
}
