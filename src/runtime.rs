use crate::{
    input::{InputController, InputEvent, InputState},
    ir::{Hat, Program},
    jit::NativeProgram,
    project::Project,
    value::{ScratchValue, number},
};
use anyhow::{Result, anyhow, ensure};
use serde::Serialize;
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    pub turbo: bool,
    pub tick_hz: f64,
    pub warp_timeout: Duration,
    pub time_limit: Option<Duration>,
    pub max_steps: Option<u64>,
}
impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            turbo: false,
            tick_hz: 30.0,
            warp_timeout: Duration::from_millis(500),
            time_limit: None,
            max_steps: None,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct SpriteState {
    pub name: String,
    pub is_stage: bool,
    pub x: f64,
    pub y: f64,
    pub direction: f64,
    pub size: f64,
    pub visible: bool,
    pub costume: usize,
    pub rotation_style: String,
}
struct AtomicSprite {
    x: AtomicU64,
    y: AtomicU64,
    direction: AtomicU64,
    size: AtomicU64,
    visible: AtomicBool,
    costume: AtomicUsize,
    rotation: AtomicUsize,
}
pub struct SharedScene {
    pub input: InputController,
    sprites: Vec<AtomicSprite>,
    pub stopped: AtomicBool,
    pub steps: AtomicU64,
    pub finished: AtomicBool,
}
impl SharedScene {
    fn new(project: &Project) -> Self {
        Self {
            input: InputController::default(),
            sprites: project
                .targets
                .iter()
                .map(|t| AtomicSprite {
                    x: AtomicU64::new(t.x.to_bits()),
                    y: AtomicU64::new(t.y.to_bits()),
                    direction: AtomicU64::new(t.direction.to_bits()),
                    size: AtomicU64::new(t.size.to_bits()),
                    visible: AtomicBool::new(t.visible),
                    costume: AtomicUsize::new(t.current_costume),
                    rotation: AtomicUsize::new(rotation_id(&t.rotation_style)),
                })
                .collect(),
            stopped: AtomicBool::new(false),
            steps: AtomicU64::new(0),
            finished: AtomicBool::new(false),
        }
    }
    pub fn snapshot(&self, project: &Project) -> Vec<SpriteState> {
        self.sprites
            .iter()
            .zip(&project.targets)
            .map(|(s, t)| SpriteState {
                name: t.name.clone(),
                is_stage: t.is_stage,
                x: f64::from_bits(s.x.load(Ordering::Relaxed)),
                y: f64::from_bits(s.y.load(Ordering::Relaxed)),
                direction: f64::from_bits(s.direction.load(Ordering::Relaxed)),
                size: f64::from_bits(s.size.load(Ordering::Relaxed)),
                visible: s.visible.load(Ordering::Relaxed),
                costume: s.costume.load(Ordering::Relaxed),
                rotation_style: rotation_name(s.rotation.load(Ordering::Relaxed)).into(),
            })
            .collect()
    }
}
fn rotation_id(s: &str) -> usize {
    match s {
        "left-right" => 1,
        "don't rotate" => 2,
        _ => 0,
    }
}
fn rotation_name(i: usize) -> &'static str {
    match i {
        1 => "left-right",
        2 => "don't rotate",
        _ => "all around",
    }
}
#[derive(Debug, Serialize)]
pub struct VariableState {
    pub target: String,
    pub id: String,
    pub name: String,
    pub value: ScratchValue,
}
#[derive(Debug, Serialize)]
pub struct ListState {
    pub target: String,
    pub id: String,
    pub name: String,
    pub items: Vec<ScratchValue>,
}
#[derive(Debug, Serialize)]
pub struct RunReport {
    pub steps: u64,
    pub yields: u64,
    pub ticks: u64,
    pub elapsed_seconds: f64,
    pub interrupted: bool,
    pub variables: Vec<VariableState>,
    pub lists: Vec<ListState>,
    pub sprites: Vec<SpriteState>,
    pub messages: Vec<String>,
}
pub struct RunningProject {
    pub scene: Arc<SharedScene>,
    worker: Option<thread::JoinHandle<Result<RunReport>>>,
}
impl RunningProject {
    pub fn stop(&self) {
        self.scene.stopped.store(true, Ordering::Relaxed);
    }
    pub fn is_finished(&self) -> bool {
        self.scene.finished.load(Ordering::Acquire)
    }
    pub fn join(mut self) -> Result<RunReport> {
        self.worker
            .take()
            .unwrap()
            .join()
            .map_err(|_| anyhow!("JIT execution thread panicked"))?
    }
}
impl Drop for RunningProject {
    fn drop(&mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
pub fn spawn(project: Arc<Project>, options: RuntimeOptions) -> Result<RunningProject> {
    spawn_mode(project, options, false)
}
/// Keep the scheduler alive for keyboard/click hats and player controls, even
/// after all current scripts finish. Stop the host through RunningProject::stop.
/// Starts idle; send InputEvent::Flag when the display is ready to run scripts.
pub fn spawn_interactive(project: Arc<Project>, options: RuntimeOptions) -> Result<RunningProject> {
    spawn_mode(project, options, true)
}
fn spawn_mode(
    project: Arc<Project>,
    options: RuntimeOptions,
    interactive: bool,
) -> Result<RunningProject> {
    ensure!(
        options.tick_hz.is_finite() && (1.0..=1000.0).contains(&options.tick_hz),
        "tick rate must be between 1 and 1000 Hz"
    );
    ensure!(
        !options.warp_timeout.is_zero() && options.warp_timeout <= Duration::from_millis(500),
        "warp watchdog must be between 1 ns and 500 ms"
    );
    let program = Program::lower(&project)?;
    let scene = Arc::new(SharedScene::new(&project));
    let shared = scene.clone();
    let worker = thread::Builder::new()
        .name("scratch-jit".into())
        .spawn(move || {
            let result = (|| {
                let native = NativeProgram::compile(&program)?;
                let mut vm = Vm::new(project, program, options, shared.clone());
                vm.interactive = interactive;
                vm.run(&native)
            })();
            if result.is_err() {
                shared.stopped.store(true, Ordering::Relaxed);
            }
            shared.finished.store(true, Ordering::Release);
            result
        })?;
    Ok(RunningProject {
        scene,
        worker: Some(worker),
    })
}

struct Frame {
    return_pc: usize,
    warp: bool,
    args: Vec<u64>,
    loops: Vec<f64>,
}
struct Fiber {
    id: u64,
    target: usize,
    pc: usize,
    frames: Vec<Frame>,
    receiver: Option<usize>,
    wake: Option<Instant>,
    waiting: HashSet<u64>,
    done: bool,
}
impl Fiber {
    fn new(id: u64, target: usize, pc: usize, loops: usize) -> Self {
        Self {
            id,
            target,
            pc,
            receiver: None,
            frames: vec![Frame {
                return_pc: 0,
                warp: false,
                args: vec![],
                loops: vec![0.0; loops],
            }],
            wake: None,
            waiting: HashSet::new(),
            done: false,
        }
    }
    fn warp(&self) -> bool {
        self.frames.last().is_some_and(|f| f.warp)
    }
}
enum Event {
    Broadcast { name: String, wait: bool },
    StopOthers,
    StopAll,
}
pub(crate) struct Vm {
    input: InputState,
    interactive: bool,
    project: Arc<Project>,
    program: Program,
    vars: Vec<u64>,
    lists: Vec<Vec<u64>>,
    current: Fiber,
    sprites: Vec<SpriteState>,
    shared: Arc<SharedScene>,
    options: RuntimeOptions,
    events: Vec<Event>,
    call_args: Vec<u64>,
    messages: VecDeque<String>,
    start: Instant,
    timer: Instant,
    slice_start: Instant,
    warp_start: Option<Instant>,
    steps: u64,
    yields: u64,
    ticks: u64,
    error: Option<String>,
    random: u64,
}
impl Vm {
    fn new(
        project: Arc<Project>,
        program: Program,
        options: RuntimeOptions,
        shared: Arc<SharedScene>,
    ) -> Self {
        let now = Instant::now();
        Self {
            input: InputState::default(),
            interactive: false,
            vars: program.initial_variables.clone(),
            lists: program.initial_lists.clone(),
            current: Fiber::new(0, 0, 0, program.loop_slots),
            sprites: shared.snapshot(&project),
            project,
            program,
            shared,
            options,
            events: vec![],
            call_args: vec![],
            messages: VecDeque::new(),
            start: now,
            timer: now,
            slice_start: now,
            warp_start: None,
            steps: 0,
            yields: 0,
            ticks: 0,
            error: None,
            random: 0x5deece66d
                ^ std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64,
        }
    }
    fn publish_sprite(&self, id: usize) {
        let s = &self.sprites[id];
        let a = &self.shared.sprites[id];
        a.x.store(s.x.to_bits(), Ordering::Relaxed);
        a.y.store(s.y.to_bits(), Ordering::Relaxed);
        a.direction.store(s.direction.to_bits(), Ordering::Relaxed);
        a.size.store(s.size.to_bits(), Ordering::Relaxed);
        a.visible.store(s.visible, Ordering::Relaxed);
        a.costume.store(s.costume, Ordering::Relaxed);
        a.rotation
            .store(rotation_id(&s.rotation_style), Ordering::Relaxed);
    }
    fn publish(&self) {
        for id in 0..self.sprites.len() {
            self.publish_sprite(id);
        }
        self.shared.steps.store(self.steps, Ordering::Relaxed);
    }
    fn run(&mut self, native: &NativeProgram) -> Result<RunReport> {
        let mut fibers: Vec<Fiber> = self
            .program
            .scripts
            .iter()
            .filter(|s| !self.interactive && matches!(s.hat, Hat::Flag))
            .enumerate()
            .map(|(i, s)| Fiber::new(i as u64 + 1, s.target, s.entry, self.program.loop_slots))
            .collect();
        let mut next_id = fibers.len() as u64 + 1;
        let tick = Duration::from_secs_f64(1.0 / self.options.tick_hz);
        let mut tick_deadline = Instant::now();
        while (!fibers.is_empty() || self.interactive)
            && !self.shared.stopped.load(Ordering::Relaxed)
        {
            if self.limit_reached() {
                self.shared.stopped.store(true, Ordering::Relaxed);
                break;
            }
            let (input, events) = self.shared.input.drain();
            self.input = input;
            for event in events {
                if matches!(event, InputEvent::Stop | InputEvent::Flag) {
                    fibers.clear();
                }
                if matches!(event, InputEvent::Flag) {
                    self.timer = Instant::now();
                }
                for (script_id, script) in self.program.scripts.iter().enumerate() {
                    let triggered = match (&event, &script.hat) {
                        (InputEvent::Flag, Hat::Flag) => true,
                        (InputEvent::Key { key, down: true }, Hat::Key(expected)) => {
                            expected.eq_ignore_ascii_case("any")
                                || crate::input::normalize_key(expected).as_ref() == Some(key)
                        }
                        (InputEvent::Click { target }, Hat::Click) => *target == script.target,
                        _ => false,
                    };
                    // Input hats do not restart an already-running instance.
                    if triggered
                        && fibers.len() < 4096
                        && !fibers
                            .iter()
                            .any(|f| f.receiver == Some(script_id) && !f.done)
                    {
                        let mut fiber = Fiber::new(
                            next_id,
                            script.target,
                            script.entry,
                            self.program.loop_slots,
                        );
                        next_id += 1;
                        fiber.receiver = Some(script_id);
                        fibers.push(fiber);
                    }
                }
            }
            self.ticks += 1;
            let mut index = 0;
            // New broadcasts become runnable during this round. Cap a round to
            // retain host responsiveness even with broadcast storms.
            let mut dispatched = 0;
            while index < fibers.len() && dispatched < 4096 {
                if self.shared.stopped.load(Ordering::Relaxed) || self.limit_reached() {
                    break;
                }
                let alive: HashSet<u64> = fibers.iter().filter(|f| !f.done).map(|f| f.id).collect();
                fibers[index].waiting.retain(|id| alive.contains(id));
                if fibers[index].done
                    || !fibers[index].waiting.is_empty()
                    || fibers[index].wake.is_some_and(|w| w > Instant::now())
                {
                    index += 1;
                    continue;
                }
                let placeholder = Fiber::new(0, 0, 0, 0);
                self.current = std::mem::replace(&mut fibers[index], placeholder);
                self.current.wake = None;
                self.slice_start = Instant::now();
                self.warp_start = self.current.warp().then(Instant::now);
                native.execute(self);
                self.yields += u64::from(!self.current.done);
                for event in std::mem::take(&mut self.events) {
                    match event {
                        Event::Broadcast { name, wait } => {
                            for (script_id, script) in self.program.scripts.iter().enumerate() {
                                if !matches!(&script.hat,Hat::Broadcast(n) if n==&name) {
                                    continue;
                                }
                                // Preserve a restarted receiver's identity so other
                                // broadcast-and-wait callers keep waiting for it.
                                let mut receiver_id = None;
                                if self.current.receiver == Some(script_id) {
                                    receiver_id = Some(self.current.id);
                                    self.current.done = true;
                                }
                                for f in &mut fibers {
                                    if !f.done && f.receiver == Some(script_id) {
                                        receiver_id = Some(f.id);
                                        f.done = true;
                                    }
                                }
                                if fibers.iter().filter(|f| !f.done).count() >= 4096 {
                                    self.error =
                                        Some("broadcast exceeds 4096 concurrent scripts".into());
                                    break;
                                }
                                let id = receiver_id.unwrap_or_else(|| {
                                    let id = next_id;
                                    next_id += 1;
                                    id
                                });
                                let mut f = Fiber::new(
                                    id,
                                    script.target,
                                    script.entry,
                                    self.program.loop_slots,
                                );
                                f.receiver = Some(script_id);
                                if wait {
                                    self.current.waiting.insert(id);
                                }
                                fibers.push(f);
                            }
                        }
                        Event::StopOthers => {
                            for f in &mut fibers {
                                if f.target == self.current.target && f.id != self.current.id {
                                    f.done = true;
                                }
                            }
                        }
                        Event::StopAll => {
                            for fiber in &mut fibers {
                                fiber.done = true;
                            }
                            self.current.done = true;
                        }
                    }
                }
                fibers[index] = std::mem::replace(&mut self.current, Fiber::new(0, 0, 0, 0));
                self.publish();
                if let Some(error) = self.error.take() {
                    return Err(anyhow!(error));
                }
                index += 1;
                dispatched += 1;
            }
            fibers.retain(|f| !f.done);
            let roots =
                self.program
                    .constants
                    .iter()
                    .copied()
                    .chain(self.vars.iter().copied())
                    .chain(self.lists.iter().flatten().copied())
                    .chain(fibers.iter().flat_map(|f| {
                        f.frames.iter().flat_map(|frame| frame.args.iter().copied())
                    }));
            self.program.values.collect(roots);
            if fibers.is_empty() && !self.interactive {
                break;
            }
            if !self.options.turbo {
                tick_deadline += tick;
                self.sleep_until(tick_deadline);
                if Instant::now() > tick_deadline + tick {
                    tick_deadline = Instant::now();
                }
            } else if fibers
                .iter()
                .all(|f| !f.waiting.is_empty() || f.wake.is_some_and(|w| w > Instant::now()))
            {
                self.sleep_until(
                    fibers
                        .iter()
                        .filter_map(|f| f.wake)
                        .min()
                        .unwrap_or(Instant::now() + Duration::from_millis(1)),
                );
            }
        }
        self.publish();
        Ok(RunReport {
            steps: self.steps,
            yields: self.yields,
            ticks: self.ticks,
            elapsed_seconds: self.start.elapsed().as_secs_f64(),
            interrupted: self.shared.stopped.load(Ordering::Relaxed),
            variables: self
                .program
                .variable_names
                .iter()
                .enumerate()
                .map(|(i, (target, id, name))| VariableState {
                    target: self.project.targets[*target].name.clone(),
                    id: id.clone(),
                    name: name.clone(),
                    value: self.program.values.decode(self.vars[i]),
                })
                .collect(),
            lists: self
                .program
                .list_names
                .iter()
                .enumerate()
                .map(|(i, (target, id, name))| ListState {
                    target: self.project.targets[*target].name.clone(),
                    id: id.clone(),
                    name: name.clone(),
                    items: self.lists[i]
                        .iter()
                        .map(|v| self.program.values.decode(*v))
                        .collect(),
                })
                .collect(),
            sprites: self.sprites.clone(),
            messages: self.messages.iter().cloned().collect(),
        })
    }
    fn limit_reached(&self) -> bool {
        self.options
            .time_limit
            .is_some_and(|d| self.start.elapsed() >= d)
            || self.options.max_steps.is_some_and(|s| self.steps >= s)
    }
    fn sleep_until(&self, deadline: Instant) {
        while Instant::now() < deadline
            && !self.shared.stopped.load(Ordering::Relaxed)
            && !self.limit_reached()
        {
            thread::sleep((deadline - Instant::now().min(deadline)).min(Duration::from_millis(2)));
        }
    }
    fn value(&self, bits: u64) -> ScratchValue {
        self.program.values.decode(bits)
    }
    fn fail(&mut self, message: &str) {
        self.error = Some(message.into());
        self.current.done = true;
    }
}
// These functions have a C ABI and are only registered inside our JIT module.
// Their pointer argument is always a live, exclusively borrowed Vm supplied by
// NativeProgram::execute. None may unwind through generated machine code.
pub(crate) extern "C" fn rt_pc(vm: &mut Vm) -> u32 {
    vm.current.pc as u32
}
pub(crate) extern "C" fn rt_checkpoint(vm: &mut Vm, pc: u32) -> u32 {
    vm.current.pc = pc as usize;
    if vm.current.done || vm.error.is_some() {
        return 1;
    }
    if vm.shared.stopped.load(Ordering::Relaxed) || vm.limit_reached() {
        vm.shared.stopped.store(true, Ordering::Relaxed);
        vm.current.done = true;
        return 1;
    }
    let now = Instant::now();
    if vm.current.warp() {
        let start = *vm.warp_start.get_or_insert(now);
        if now.duration_since(start) >= vm.options.warp_timeout {
            vm.fail("warp watchdog interrupted execution after its time budget (maximum 500 ms)");
            return 1;
        }
    } else {
        vm.warp_start = None;
        if now.duration_since(vm.slice_start) >= Duration::from_millis(2) {
            return 1;
        }
    }
    vm.steps += 1;
    if vm.steps & 1023 == 0 {
        vm.shared.steps.store(vm.steps, Ordering::Relaxed);
    }
    0
}
pub(crate) extern "C" fn rt_boundary(vm: &mut Vm, next: u32) -> u32 {
    vm.current.pc = next as usize;
    u32::from(!vm.current.warp() && !vm.options.turbo)
}
pub(crate) extern "C" fn rt_number(vm: &mut Vm, value: u64) -> f64 {
    vm.value(value).to_number()
}
pub(crate) extern "C" fn rt_bool(vm: &mut Vm, value: u64) -> u32 {
    u32::from(vm.value(value).to_bool())
}
pub(crate) extern "C" fn rt_get(vm: &mut Vm, slot: u32) -> u64 {
    vm.vars[slot as usize]
}
pub(crate) extern "C" fn rt_set(vm: &mut Vm, slot: u32, value: u64) {
    vm.vars[slot as usize] = value;
}
fn list_index(value: &ScratchValue, length: usize, random: &mut u64) -> Option<usize> {
    let index = match value {
        ScratchValue::String(s) if s == "last" => length as f64,
        ScratchValue::String(s) if s == "random" || s == "any" => {
            if length == 0 {
                return None;
            }
            *random ^= *random << 13;
            *random ^= *random >> 7;
            *random ^= *random << 17;
            return Some((*random % length as u64) as usize);
        }
        _ => value.to_number().floor(),
    };
    (index >= 1.0 && index <= length as f64).then(|| index as usize - 1)
}
pub(crate) extern "C" fn rt_list(vm: &mut Vm, op: u32, slot: u32, a: u64, b: u64) -> u64 {
    const LIMIT: usize = 200_000;
    let slot = slot as usize;
    let arg = vm.value(a);
    let length = vm.lists[slot].len();
    let result = match op {
        0 => {
            if length < LIMIT {
                vm.lists[slot].push(a);
            }
            ScratchValue::Number(0.0)
        }
        1 => {
            if matches!(&arg, ScratchValue::String(s) if s == "all") {
                vm.lists[slot].clear();
            } else if let Some(index) = list_index(&arg, length, &mut vm.random) {
                vm.lists[slot].remove(index);
            }
            ScratchValue::Number(0.0)
        }
        2 => {
            vm.lists[slot].clear();
            ScratchValue::Number(0.0)
        }
        3 => {
            if let Some(index) = list_index(&arg, length + 1, &mut vm.random).filter(|i| *i < LIMIT)
            {
                vm.lists[slot].insert(index, b);
                if vm.lists[slot].len() > LIMIT {
                    vm.lists[slot].pop();
                }
            }
            ScratchValue::Number(0.0)
        }
        4 => {
            if let Some(index) = list_index(&arg, length, &mut vm.random) {
                vm.lists[slot][index] = b;
            }
            ScratchValue::Number(0.0)
        }
        5 => {
            return list_index(&arg, length, &mut vm.random)
                .map(|i| vm.lists[slot][i])
                .unwrap_or_else(|| {
                    vm.program
                        .values
                        .encode(ScratchValue::String(String::new()))
                });
        }
        6 => ScratchValue::Number(length as f64),
        7 => {
            let values: Vec<_> = vm.lists[slot].iter().map(|v| vm.value(*v)).collect();
            let single = values
                .iter()
                .all(|v| matches!(v, ScratchValue::String(s) if s.encode_utf16().count() == 1));
            ScratchValue::String(
                values
                    .iter()
                    .map(ScratchValue::text)
                    .collect::<Vec<_>>()
                    .join(if single { "" } else { " " }),
            )
        }
        8 | 9 => {
            let index = vm.lists[slot]
                .iter()
                .position(|v| vm.value(*v).compare(&arg).is_eq());
            if op == 9 {
                ScratchValue::Boolean(index.is_some())
            } else {
                ScratchValue::Number(index.map_or(0.0, |i| (i + 1) as f64))
            }
        }
        _ => ScratchValue::Number(0.0),
    };
    vm.program.values.encode(result)
}
pub(crate) extern "C" fn rt_arg(vm: &mut Vm, slot: u32) -> u64 {
    vm.current.frames.last().unwrap().args[slot as usize]
}
pub(crate) extern "C" fn rt_push_arg(vm: &mut Vm, value: u64) {
    vm.call_args.push(value);
}
pub(crate) extern "C" fn rt_call(vm: &mut Vm, procedure: u32, next: u32) -> u32 {
    if vm.current.frames.len() >= 1024 {
        vm.fail("custom block recursion exceeds 1024 frames");
        return 1;
    }
    let p = &vm.program.procedures[procedure as usize];
    let warp = p.warp || vm.current.warp();
    vm.current.pc = p.entry;
    vm.current.frames.push(Frame {
        return_pc: next as usize,
        warp,
        args: std::mem::take(&mut vm.call_args),
        loops: vec![0.0; vm.program.loop_slots],
    });
    0
}
pub(crate) extern "C" fn rt_return(vm: &mut Vm) -> u32 {
    let frame = vm.current.frames.pop().unwrap();
    if vm.current.frames.is_empty() {
        vm.current.done = true;
        1
    } else {
        vm.current.pc = frame.return_pc;
        0
    }
}
pub(crate) extern "C" fn rt_loop_set(vm: &mut Vm, slot: u32, count: f64) {
    vm.current.frames.last_mut().unwrap().loops[slot as usize] = (count + 0.5).floor().max(0.0);
}
pub(crate) extern "C" fn rt_loop_get(vm: &mut Vm, slot: u32) -> f64 {
    vm.current.frames.last().unwrap().loops[slot as usize]
}
pub(crate) extern "C" fn rt_loop_dec(vm: &mut Vm, slot: u32) {
    vm.current.frames.last_mut().unwrap().loops[slot as usize] -= 1.0;
}
pub(crate) extern "C" fn rt_wait(vm: &mut Vm, seconds: f64, next: u32) {
    let seconds = if seconds.is_nan() {
        0.0
    } else {
        seconds.clamp(0.0, 86400.0 * 365.0)
    };
    vm.current.wake = Some(Instant::now() + Duration::from_secs_f64(seconds));
    vm.current.pc = next as usize;
}
pub(crate) extern "C" fn rt_broadcast(vm: &mut Vm, name: u64, wait: u32, next: u32) {
    if vm.events.len() >= 4096 {
        vm.fail("broadcast exceeds 4096 pending events");
        return;
    }
    vm.events.push(Event::Broadcast {
        name: vm.value(name).text(),
        wait: wait != 0,
    });
    vm.current.pc = next as usize;
}
pub(crate) extern "C" fn rt_stop(vm: &mut Vm, mode: u32) {
    match mode {
        0 => {
            if vm.interactive {
                vm.events.push(Event::StopAll);
            } else {
                vm.shared.stopped.store(true, Ordering::Relaxed);
            }
            vm.current.done = true;
        }
        1 => vm.current.done = true,
        _ => vm.events.push(Event::StopOthers),
    }
}
pub(crate) extern "C" fn rt_binary(vm: &mut Vm, op: u32, a: u64, b: u64) -> u64 {
    let a = vm.value(a);
    let b = vm.value(b);
    let result = match op {
        4 => {
            let n = a.to_number();
            let d = b.to_number();
            ScratchValue::Number(((n % d) + d) % d)
        }
        5 => ScratchValue::Boolean(a.compare(&b).is_lt()),
        6 => ScratchValue::Boolean(a.compare(&b).is_eq()),
        7 => ScratchValue::Boolean(a.compare(&b).is_gt()),
        8 => ScratchValue::Boolean(a.to_bool() && b.to_bool()),
        9 => ScratchValue::Boolean(a.to_bool() || b.to_bool()),
        10 => {
            let mut s = a.text();
            s.push_str(&b.text());
            if s.len() > 1_048_576 {
                vm.fail("string exceeds 1 MiB");
                s.clear();
            }
            ScratchValue::String(s)
        }
        11 => ScratchValue::Boolean(a.text().to_lowercase().contains(&b.text().to_lowercase())),
        12 => {
            let index = a.to_number().floor() - 1.0;
            ScratchValue::String(if (0.0..1_048_576.0).contains(&index) {
                b.text()
                    .chars()
                    .nth(index as usize)
                    .map(String::from)
                    .unwrap_or_default()
            } else {
                String::new()
            })
        }
        13 => {
            vm.random ^= vm.random << 13;
            vm.random ^= vm.random >> 7;
            vm.random ^= vm.random << 17;
            let mut lo = a.to_number();
            let mut hi = b.to_number();
            if lo > hi {
                std::mem::swap(&mut lo, &mut hi);
            }
            let r = (vm.random >> 11) as f64 / (1u64 << 53) as f64;
            ScratchValue::Number(if lo.fract() == 0.0 && hi.fract() == 0.0 {
                lo + (r * (hi - lo + 1.0)).floor()
            } else {
                lo + r * (hi - lo)
            })
        }
        _ => ScratchValue::Number(0.0),
    };
    vm.program.values.encode(result)
}
pub(crate) extern "C" fn rt_unary(vm: &mut Vm, op: u32, a: u64) -> u64 {
    let a = vm.value(a);
    let n = a.to_number();
    let result = match op {
        0 => ScratchValue::Boolean(!a.to_bool()),
        1 => ScratchValue::Number(a.text().chars().count() as f64),
        17 => ScratchValue::Boolean(vm.input.key_down(&a.text())),
        _ => ScratchValue::Number(match op {
            2 => (n + 0.5).floor(),
            3 => n.abs(),
            4 => n.floor(),
            5 => n.ceil(),
            6 => n.sqrt(),
            7 => n.to_radians().sin(),
            8 => n.to_radians().cos(),
            9 => n.to_radians().tan(),
            10 => n.asin().to_degrees(),
            11 => n.acos().to_degrees(),
            12 => n.atan().to_degrees(),
            13 => n.ln(),
            14 => n.log10(),
            15 => n.exp(),
            16 => 10f64.powf(n),
            _ => 0.0,
        }),
    };
    vm.program.values.encode(result)
}
pub(crate) extern "C" fn rt_state(vm: &mut Vm, field: u32) -> u64 {
    if field == 9 {
        return vm
            .program
            .values
            .encode(ScratchValue::Boolean(vm.input.down));
    }
    let s = &vm.sprites[vm.current.target];
    if field == 6 {
        let name = vm.project.targets[vm.current.target]
            .costumes
            .get(s.costume)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        return vm.program.values.encode(ScratchValue::String(name));
    }
    number(match field {
        0 => s.x,
        1 => s.y,
        2 => s.direction,
        3 => s.size,
        4 => vm.timer.elapsed().as_secs_f64(),
        5 => s.costume as f64 + 1.0,
        7 => vm.input.x,
        8 => vm.input.y,
        _ => 0.0,
    })
}
pub(crate) extern "C" fn rt_action(vm: &mut Vm, op: u32, a: u64, b: u64) {
    let value = vm.value(a);
    let n = value.to_number();
    let m = vm.value(b).to_number();
    let target = vm.current.target;
    let s = &mut vm.sprites[target];
    match op {
        0 => {
            let d = s.direction.to_radians();
            s.x += n * d.sin();
            s.y += n * d.cos();
        }
        1 => {
            s.x = n;
            s.y = m;
        }
        2 => s.x = n,
        3 => s.y = n,
        4 => s.x += n,
        5 => s.y += n,
        6 => s.direction = n,
        7 => s.direction += n,
        8 => s.direction -= n,
        9 => s.visible = true,
        10 => s.visible = false,
        11 => s.size = n.max(0.0),
        12 => s.size = (s.size + n).max(0.0),
        13 => {
            if vm.messages.len() >= 1024 {
                vm.messages.pop_front();
            }
            vm.messages
                .push_back(format!("{}: {}", s.name, value.text()));
        }
        14 | 15 => {
            let costumes = &vm.project.targets[target].costumes;
            if !costumes.is_empty() {
                if op == 15 {
                    s.costume = (s.costume + 1) % costumes.len();
                } else if let Some(id) = costumes.iter().position(|c| c.name == value.text()) {
                    s.costume = id;
                } else {
                    s.costume = ((n.round() - 1.0).rem_euclid(costumes.len() as f64)) as usize;
                }
            }
        }
        16 => vm.timer = Instant::now(),
        17 => s.rotation_style = value.text(),
        _ => {}
    }
    if matches!(op, 6..=8) {
        s.direction = (s.direction + 180.0).rem_euclid(360.0) - 180.0;
        if s.direction == -180.0 {
            s.direction = 180.0;
        }
    }
    vm.publish_sprite(target);
}
