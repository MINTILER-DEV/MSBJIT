use crate::{
    audio::{AudioPlayer, SoundState},
    input::{InputController, InputEvent, InputState},
    ir::{Hat, Program},
    jit::NativeProgram,
    lists::{ListView, RuntimeList},
    pen::{MAX_QUEUED_BYTES, PenCanvas, PenState},
    project::Project,
    sprite_cpu::CostumePixels,
    value::{ScratchValue, number},
};
use anyhow::{Context, Result, anyhow, ensure};
use serde::Serialize;
#[cfg(feature = "graphics")]
use std::collections::HashMap;
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub const DEFAULT_USERNAME: &str = "Player1";
pub const DEFAULT_LANGUAGE: &str = "English";
pub(crate) const MAX_CLONES: usize = 300;

#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    pub username: String,
    pub language: String,
    pub turbo: bool,
    pub tick_hz: f64,
    pub warp_timeout: Duration,
    pub time_limit: Option<Duration>,
    pub max_steps: Option<u64>,
}
impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            username: DEFAULT_USERNAME.into(),
            language: DEFAULT_LANGUAGE.into(),
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
    pub id: usize,
    pub target: usize,
    pub is_clone: bool,
    pub active: bool,
    pub layer_order: i64,
    pub effects: crate::effects::GraphicEffects,
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
    layer_order: AtomicU64,
    effects: [AtomicU64; 7],
    x: AtomicU64,
    y: AtomicU64,
    direction: AtomicU64,
    size: AtomicU64,
    visible: AtomicBool,
    costume: AtomicUsize,
    rotation: AtomicUsize,
}
#[derive(Debug, Clone, Serialize)]
pub struct QuestionPrompt {
    pub id: u64,
    pub text: String,
    pub draft: String,
}
pub struct SharedScene {
    pub question: Mutex<Option<QuestionPrompt>>,
    pub(crate) pen: Mutex<PenCanvas>,
    gpu_pen: AtomicBool,
    pub(crate) monitors: Mutex<Vec<crate::monitors::Monitor>>,
    pub input: InputController,
    sprites: Vec<AtomicSprite>,
    clones: Mutex<Vec<SpriteState>>,
    pub stopped: AtomicBool,
    pub steps: AtomicU64,
    pub finished: AtomicBool,
}
impl SharedScene {
    fn new(project: &Project) -> Self {
        Self {
            question: Mutex::new(None),
            pen: Mutex::new(PenCanvas::default()),
            gpu_pen: AtomicBool::new(false),
            monitors: Mutex::new(crate::monitors::create(project)),
            input: InputController::default(),
            clones: Mutex::new(Vec::new()),
            sprites: project
                .targets
                .iter()
                .map(|t| AtomicSprite {
                    layer_order: AtomicU64::new(t.layer_order as i64 as u64),
                    effects: std::array::from_fn(|_| AtomicU64::new(0)),
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
    /// Enable command-based pen rendering before sending the initial flag.
    #[cfg(feature = "graphics")]
    pub(crate) fn enable_gpu_pen(&self) {
        self.gpu_pen.store(true, Ordering::Release);
    }
    pub fn snapshot(&self, project: &Project) -> Vec<SpriteState> {
        let mut states: Vec<_> = self
            .sprites
            .iter()
            .zip(&project.targets)
            .enumerate()
            .map(|(id, (s, t))| SpriteState {
                id,
                target: id,
                is_clone: false,
                active: true,
                layer_order: s.layer_order.load(Ordering::Relaxed) as i64,
                effects: crate::effects::GraphicEffects::from_values(std::array::from_fn(|i| {
                    f64::from_bits(s.effects[i].load(Ordering::Relaxed))
                })),
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
            .collect();
        states.extend(
            self.clones
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .cloned(),
        );
        states
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
    glide: Option<Glide>,
    sound_wait: Option<u64>,
    ask_wait: Option<u64>,
}
struct Glide {
    start: Instant,
    duration: f64,
    from: (f64, f64),
    to: (f64, f64),
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
            glide: None,
            sound_wait: None,
            ask_wait: None,
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
    CloneStart(usize),
    DeleteClone(usize),
}
struct CloneData {
    vars: Vec<u64>,
    lists: Vec<RuntimeList>,
    views: Vec<ListView>,
}
fn refresh_waiters(fibers: &mut [Fiber]) {
    if fibers.iter().any(|f| !f.waiting.is_empty()) {
        let alive: HashSet<_> = fibers.iter().filter(|f| !f.done).map(|f| f.id).collect();
        for fiber in fibers {
            fiber.waiting.retain(|id| alive.contains(id));
        }
    }
}
/// Stable prefix shared with the JIT. Variable and list-view buffers never
/// resize; helpers refresh list data pointers and current argument pointers.
#[repr(C)]
pub(crate) struct JitState {
    pub vars: *mut u64,
    pub lists: *mut ListView,
    pub args: *const u64,
    pub steps: u64,
    pub budget: u32,
    pub pc: u32,
    pub yield_on_loop: u32,
    pub local_vars: *mut u64,
    pub local_lists: *mut ListView,
    pub loops: *mut f64,
    pub numeric_strings: *const f64,
}
#[repr(C)]
pub(crate) struct Vm {
    jit: JitState,
    input: InputState,
    interactive: bool,
    project: Arc<Project>,
    program: Program,
    vars: Vec<u64>,
    lists: Vec<RuntimeList>,
    list_views: Vec<ListView>,
    clones: Vec<Option<CloneData>>,
    next_sprite_id: usize,
    pens: Vec<PenState>,
    pen: PenCanvas,
    audio: AudioPlayer,
    sound_states: Vec<SoundState>,
    questions: VecDeque<QuestionPrompt>,
    next_question: u64,
    answer: String,
    #[cfg(feature = "graphics")]
    costume_pixels: HashMap<(usize, usize), Arc<CostumePixels>>,
    #[cfg(feature = "graphics")]
    costume_bytes: usize,
    current: Fiber,
    sprites: Vec<SpriteState>,
    shared: Arc<SharedScene>,
    options: RuntimeOptions,
    events: Vec<Event>,
    call_args: Vec<u64>,
    spare_frames: Vec<Frame>,
    messages: VecDeque<String>,
    start: Instant,
    timer: Instant,
    slice_start: Instant,
    monitor_published: Instant,
    warp_start: Option<Instant>,
    yields: u64,
    ticks: u64,
    error: Option<String>,
    random: u64,
}
impl Vm {
    fn new(
        project: Arc<Project>,
        mut program: Program,
        options: RuntimeOptions,
        shared: Arc<SharedScene>,
    ) -> Self {
        let now = Instant::now();
        let mut vars = std::mem::take(&mut program.initial_variables);
        let mut lists: Vec<_> = program
            .initial_lists
            .drain(..)
            .map(RuntimeList::new)
            .collect();
        let mut list_views: Vec<_> = lists.iter_mut().map(RuntimeList::view).collect();
        Self {
            jit: JitState {
                vars: vars.as_mut_ptr(),
                lists: list_views.as_mut_ptr(),
                args: std::ptr::null(),
                steps: 0,
                budget: 0,
                pc: 0,
                yield_on_loop: 0,
                local_vars: vars.as_mut_ptr(),
                local_lists: list_views.as_mut_ptr(),
                loops: std::ptr::null_mut(),
                numeric_strings: program.values.numeric_cache_ptr(),
            },
            input: InputState::default(),
            interactive: false,
            vars,
            lists,
            list_views,
            clones: Vec::new(),
            next_sprite_id: project.targets.len(),
            pens: vec![PenState::default(); project.targets.len()],
            pen: PenCanvas::default(),
            audio: AudioPlayer::default(),
            questions: VecDeque::new(),
            next_question: 0,
            answer: String::new(),
            sound_states: project
                .targets
                .iter()
                .map(|t| SoundState {
                    volume: t.volume.clamp(0.0, 100.0),
                    ..Default::default()
                })
                .collect(),
            #[cfg(feature = "graphics")]
            costume_pixels: HashMap::new(),
            #[cfg(feature = "graphics")]
            costume_bytes: 0,
            current: Fiber::new(0, 0, 0, program.loop_slots),
            sprites: shared.snapshot(&project),
            project,
            program,
            shared,
            options,
            events: vec![],
            call_args: vec![],
            spare_frames: vec![],
            messages: VecDeque::new(),
            start: now,
            timer: now,
            slice_start: now,
            monitor_published: now,
            warp_start: None,
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
    fn publish_question(&self) {
        let mut prompt = self
            .shared
            .question
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if prompt.as_ref().map(|q| q.id) != self.questions.front().map(|q| q.id) {
            *prompt = self.questions.front().cloned();
        }
    }
    fn clear_effects(&mut self) {
        for (id, state) in self.sound_states.iter_mut().enumerate() {
            state.pitch = 0.0;
            state.pan = 0.0;
            self.audio.configure(id, *state);
        }
        for s in &mut self.sprites {
            s.effects = Default::default();
        }
        for id in 0..self.sprites.len() {
            self.publish_effects(id);
        }
    }
    fn bind_instance(&mut self, target: usize) {
        if let Some(data) = target
            .checked_sub(self.project.targets.len())
            .and_then(|i| self.clones[i].as_mut())
        {
            self.jit.local_vars = data.vars.as_mut_ptr();
            self.jit.local_lists = data.views.as_mut_ptr();
        } else {
            self.jit.local_vars = self.vars.as_mut_ptr();
            self.jit.local_lists = self.list_views.as_mut_ptr();
        }
    }
    fn create_clone(&mut self, name: &str) {
        if self.clones.iter().flatten().count() >= MAX_CLONES {
            return;
        }
        let source = if name == "_myself_" {
            Some(self.current.target)
        } else {
            self.project
                .targets
                .iter()
                .position(|t| !t.is_stage && t.name == name)
        };
        let Some(source) = source.filter(|&i| self.sprites[i].active && !self.sprites[i].is_stage)
        else {
            return;
        };
        let origin = self.sprites[source].target;
        let source_data = source
            .checked_sub(self.project.targets.len())
            .and_then(|i| self.clones[i].as_ref());
        let mut vars = vec![0; self.vars.len()];
        for (i, (owner, _, _)) in self.program.variable_names.iter().enumerate() {
            if *owner == origin {
                vars[i] = source_data.map_or(self.vars[i], |c| c.vars[i]);
            }
        }
        let mut lists: Vec<_> = self
            .program
            .list_names
            .iter()
            .enumerate()
            .map(|(i, (owner, _, _))| {
                if *owner == origin {
                    source_data.map_or_else(|| self.lists[i].clone(), |c| c.lists[i].clone())
                } else {
                    RuntimeList::new(vec![])
                }
            })
            .collect();
        let views = lists.iter_mut().map(RuntimeList::view).collect();
        let data = CloneData { vars, lists, views };
        let mut sprite = self.sprites[source].clone();
        sprite.id = self.next_sprite_id;
        self.next_sprite_id += 1;
        sprite.is_clone = true;
        let layer = sprite.layer_order;
        for s in &mut self.sprites {
            if s.active && !s.is_stage && s.layer_order >= layer {
                s.layer_order += 1;
            }
        }
        let pen = self.pens[source].clone();
        let sound_state = self.sound_states[source];
        let index = if let Some(i) = self.clones.iter().position(Option::is_none) {
            let target = self.project.targets.len() + i;
            self.clones[i] = Some(data);
            self.sprites[target] = sprite;
            self.pens[target] = pen;
            self.sound_states[target] = sound_state;
            target
        } else {
            let target = self.sprites.len();
            self.clones.push(Some(data));
            self.sprites.push(sprite);
            self.pens.push(pen);
            self.sound_states.push(sound_state);
            target
        };
        for i in 0..self.sprites.len() {
            self.publish_sprite(i);
        }
        self.events.push(Event::CloneStart(index));
    }
    fn delete_clone(&mut self, target: usize) {
        if target < self.project.targets.len() {
            return;
        }
        self.sprites[target].active = false;
        self.sprites[target].visible = false;
        self.clones[target - self.project.targets.len()] = None;
        self.pens[target].down = false;
        self.audio.stop_target(target);
        self.publish_sprite(target);
    }
    fn clear_clones(&mut self) {
        for i in self.project.targets.len()..self.sprites.len() {
            self.delete_clone(i);
        }
    }
    fn layer(&mut self, target: usize, front: bool) {
        if self.sprites[target].is_stage {
            return;
        }
        let layers = self
            .sprites
            .iter()
            .filter(|s| s.active && !s.is_stage)
            .map(|s| s.layer_order);
        self.sprites[target].layer_order = if front {
            layers.max().unwrap_or(0) + 1
        } else {
            layers.min().unwrap_or(0) - 1
        };
        self.publish_sprite(target);
    }
    fn move_to(&mut self, target: usize, x: f64, y: f64) {
        let old = (self.sprites[target].x, self.sprites[target].y);
        self.sprites[target].x = x;
        self.sprites[target].y = y;
        if self.pens[target].down {
            self.pen.line(&self.pens[target], old, (x, y));
        }
        self.publish_sprite(target);
    }
    fn named_sprite(&self, name: &str) -> Option<usize> {
        if name == "_myself_" {
            return Some(self.current.target);
        }
        if name == "_stage_" {
            return Some(self.program.stage);
        }
        self.project.targets.iter().position(|t| t.name == name)
    }
    fn destination(&mut self, name: &str) -> Option<(f64, f64)> {
        match name {
            "_mouse_" => Some((self.input.x, self.input.y)),
            "_random_" => {
                self.random ^= self.random << 13;
                self.random ^= self.random >> 7;
                self.random ^= self.random << 17;
                let x = (self.random % 481) as f64 - 240.0;
                self.random ^= self.random << 13;
                self.random ^= self.random >> 7;
                self.random ^= self.random << 17;
                Some((x, (self.random % 361) as f64 - 180.0))
            }
            _ => self
                .named_sprite(name)
                .map(|i| (self.sprites[i].x, self.sprites[i].y)),
        }
    }
    fn relative_layer(&mut self, delta: f64) {
        let target = self.current.target;
        if self.sprites[target].is_stage {
            return;
        }
        let mut order: Vec<_> = self
            .sprites
            .iter()
            .enumerate()
            .filter_map(|(i, s)| (s.active && !s.is_stage).then_some(i))
            .collect();
        order.sort_by_key(|&i| self.sprites[i].layer_order);
        let Some(old) = order.iter().position(|&i| i == target) else {
            return;
        };
        order.remove(old);
        let new = (old as f64 + delta).clamp(0.0, order.len() as f64) as usize;
        order.insert(new, target);
        for (layer, id) in order.into_iter().enumerate() {
            self.sprites[id].layer_order = layer as i64;
            self.publish_sprite(id);
        }
    }
    fn property(&mut self, property: &str, object: &str) -> u64 {
        let Some(target) = self.named_sprite(object) else {
            return number(0.0);
        };
        let sprite = &self.sprites[target];
        let value = match property {
            "x position" => ScratchValue::Number(sprite.x),
            "y position" => ScratchValue::Number(sprite.y),
            "direction" => ScratchValue::Number(sprite.direction),
            "size" => ScratchValue::Number(sprite.size),
            "volume" => ScratchValue::Number(self.sound_states[target].volume),
            "costume #" | "backdrop #" => ScratchValue::Number(sprite.costume as f64 + 1.0),
            "costume name" | "backdrop name" => ScratchValue::String(
                self.project.targets[sprite.target]
                    .costumes
                    .get(sprite.costume)
                    .map(|c| c.name.clone())
                    .unwrap_or_default(),
            ),
            _ => {
                let Some(slot) = self
                    .program
                    .variable_names
                    .iter()
                    .position(|(owner, _, name)| *owner == sprite.target && name == property)
                else {
                    return number(0.0);
                };
                return if target >= self.project.targets.len() {
                    self.clones[target - self.project.targets.len()]
                        .as_ref()
                        .unwrap()
                        .vars[slot]
                } else {
                    self.vars[slot]
                };
            }
        };
        self.program.values.encode(value)
    }
    fn publish_effects(&self, id: usize) {
        if id >= self.project.targets.len() {
            self.publish_sprite(id);
            return;
        }
        let s = &self.sprites[id];
        let a = &self.shared.sprites[id];
        for (value, atomic) in s.effects.values().into_iter().zip(&a.effects) {
            atomic.store(value.to_bits(), Ordering::Relaxed);
        }
    }
    fn publish_sprite(&self, id: usize) {
        let s = &self.sprites[id];
        if id >= self.project.targets.len() {
            let mut clones = self.shared.clones.lock().unwrap_or_else(|e| e.into_inner());
            let i = id - self.project.targets.len();
            if i == clones.len() {
                clones.push(s.clone());
            } else if let Some(slot) = clones.get_mut(i) {
                *slot = s.clone();
            }
            return;
        }
        let a = &self.shared.sprites[id];
        a.layer_order.store(s.layer_order as u64, Ordering::Relaxed);
        a.x.store(s.x.to_bits(), Ordering::Relaxed);
        a.y.store(s.y.to_bits(), Ordering::Relaxed);
        a.direction.store(s.direction.to_bits(), Ordering::Relaxed);
        a.size.store(s.size.to_bits(), Ordering::Relaxed);
        a.visible.store(s.visible, Ordering::Relaxed);
        a.costume.store(s.costume, Ordering::Relaxed);
        a.rotation
            .store(rotation_id(&s.rotation_style), Ordering::Relaxed);
    }
    fn publish(&mut self) {
        for id in 0..self.sprites.len() {
            self.publish_sprite(id);
        }
        self.shared.steps.store(self.jit.steps, Ordering::Relaxed);
        if self.monitor_published.elapsed() >= Duration::from_millis(16) {
            self.publish_monitors();
        }
    }
    fn publish_monitors(&mut self) {
        let mut monitors = self
            .shared
            .monitors
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for m in monitors.iter_mut().filter(|m| m.visible) {
            m.update(self.program.values.decode(self.vars[m.slot]));
        }
        self.monitor_published = Instant::now();
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
        let mut pen_published = Instant::now();
        while (!fibers.is_empty() || self.interactive)
            && !self.shared.stopped.load(Ordering::Relaxed)
        {
            self.audio.tick();
            if self.shared.gpu_pen.load(Ordering::Acquire) {
                self.pen.enable_gpu();
            }
            // Keep monitors fresh after the last script finishes as well as
            // during execution; an interactive player can remain idle here.
            if self.monitor_published.elapsed() >= Duration::from_millis(16) {
                self.publish_monitors();
            }
            if self.limit_reached() {
                self.shared.stopped.store(true, Ordering::Relaxed);
                break;
            }
            let (input, events) = self.shared.input.drain();
            self.input = input;
            for event in events {
                if let InputEvent::Answer { id, text } = event {
                    if self.questions.front().is_some_and(|q| q.id == id) {
                        self.answer = text;
                        self.questions.pop_front();
                        self.publish_question();
                    }
                    continue;
                }
                if let InputEvent::MonitorSlider { slot, value } = event {
                    let mut monitors = self
                        .shared
                        .monitors
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if let Some(m) = monitors.iter_mut().find(|m| m.slot == slot)
                        && let Some(value) = m.slider_value(value)
                    {
                        self.vars[slot] = number(value);
                        m.update(ScratchValue::Number(value));
                    }
                    continue;
                }
                if matches!(event, InputEvent::Stop | InputEvent::Flag) {
                    fibers.clear();
                    self.questions.clear();
                    self.publish_question();
                    self.audio.stop();
                    self.clear_clones();
                    self.clear_effects();
                }
                if matches!(event, InputEvent::Flag) {
                    self.answer.clear();
                    self.timer = Instant::now();
                }
                for (script_id, script) in self.program.scripts.iter().enumerate() {
                    let triggered = match (&event, &script.hat) {
                        (InputEvent::Flag, Hat::Flag) => true,
                        (InputEvent::Key { key, down: true }, Hat::Key(expected)) => {
                            expected.eq_ignore_ascii_case("any")
                                || crate::input::normalize_key(expected).as_ref() == Some(key)
                        }
                        (InputEvent::Click { .. }, Hat::Click) => true,
                        _ => false,
                    };
                    if !triggered {
                        continue;
                    }
                    for (target, sprite) in self.sprites.iter().enumerate() {
                        if !sprite.active
                            || sprite.target != script.target
                            || matches!(event, InputEvent::Flag) && sprite.is_clone
                        {
                            continue;
                        }
                        if let InputEvent::Click { target: id } = &event
                            && *id != sprite.id
                        {
                            continue;
                        }
                        if fibers.len() < 4096
                            && !fibers.iter().any(|f| {
                                f.receiver == Some(script_id) && f.target == target && !f.done
                            })
                        {
                            let mut f =
                                Fiber::new(next_id, target, script.entry, self.program.loop_slots);
                            next_id += 1;
                            f.receiver = Some(script_id);
                            fibers.push(f);
                        }
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
                // Most scripts have no dependencies. Avoid allocating a set of
                // every fiber on each dispatch just to check an empty wait set.
                if !fibers[index].waiting.is_empty() {
                    let mut waiting = std::mem::take(&mut fibers[index].waiting);
                    waiting.retain(|id| fibers.iter().any(|f| !f.done && f.id == *id));
                    fibers[index].waiting = waiting;
                }
                if fibers[index].done
                    || !fibers[index].waiting.is_empty()
                    || fibers[index].wake.is_some_and(|w| w > Instant::now())
                {
                    index += 1;
                    continue;
                }
                if let Some(sound) = fibers[index].sound_wait {
                    if self.audio.playing(sound) {
                        index += 1;
                        continue;
                    }
                    fibers[index].sound_wait = None;
                }
                if let Some(id) = fibers[index].ask_wait {
                    if self.questions.iter().any(|q| q.id == id) {
                        index += 1;
                        continue;
                    }
                    fibers[index].ask_wait = None;
                }
                std::mem::swap(&mut self.current, &mut fibers[index]);
                if let Some(glide) = &self.current.glide {
                    let progress = (glide.start.elapsed().as_secs_f64() / glide.duration).min(1.0);
                    let (x, y) = (
                        glide.from.0 + (glide.to.0 - glide.from.0) * progress,
                        glide.from.1 + (glide.to.1 - glide.from.1) * progress,
                    );
                    self.move_to(self.current.target, x, y);
                    if progress < 1.0 {
                        self.current.wake = Some(Instant::now() + tick);
                        std::mem::swap(&mut self.current, &mut fibers[index]);
                        index += 1;
                        continue;
                    }
                    self.current.glide = None;
                }
                self.bind_instance(self.current.target);
                self.current.wake = None;
                self.slice_start = Instant::now();
                self.warp_start = self.current.warp().then(Instant::now);
                self.jit.args = self.current.frames.last().unwrap().args.as_ptr();
                self.jit.loops = self.current.frames.last_mut().unwrap().loops.as_mut_ptr();
                self.jit.budget = 0;
                self.jit.pc = self.current.pc as u32;
                self.jit.yield_on_loop = u32::from(!self.current.warp() && !self.options.turbo);
                native.execute(self);
                self.current.pc = self.jit.pc as usize;
                self.yields += u64::from(!self.current.done);
                for event in std::mem::take(&mut self.events) {
                    match event {
                        Event::Broadcast { name, wait } => {
                            for (script_id, script) in self.program.scripts.iter().enumerate() {
                                if !matches!(&script.hat,Hat::Broadcast(n) if n==&name) {
                                    continue;
                                }
                                for (target, sprite) in self.sprites.iter().enumerate() {
                                    if !sprite.active || sprite.target != script.target {
                                        continue;
                                    }
                                    let mut receiver_id = None;
                                    if self.current.receiver == Some(script_id)
                                        && self.current.target == target
                                    {
                                        receiver_id = Some(self.current.id);
                                        self.current.done = true;
                                    }
                                    for f in &mut fibers {
                                        if !f.done
                                            && f.receiver == Some(script_id)
                                            && f.target == target
                                        {
                                            receiver_id = Some(f.id);
                                            f.done = true;
                                        }
                                    }
                                    if fibers.iter().filter(|f| !f.done).count() >= 4096 {
                                        self.error = Some(
                                            "broadcast exceeds 4096 concurrent scripts".into(),
                                        );
                                        break;
                                    }
                                    let id = receiver_id.unwrap_or_else(|| {
                                        let id = next_id;
                                        next_id += 1;
                                        id
                                    });
                                    let mut f = Fiber::new(
                                        id,
                                        target,
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
                        }

                        Event::StopOthers => {
                            for f in &mut fibers {
                                if f.target == self.current.target && f.id != self.current.id {
                                    f.done = true;
                                }
                            }
                        }
                        Event::StopAll => {
                            self.audio.stop();
                            self.clear_clones();
                            self.clear_effects();
                            for fiber in &mut fibers {
                                fiber.done = true;
                            }
                            self.current.done = true;
                        }
                        Event::CloneStart(target) => {
                            if self.sprites[target].active {
                                for script in &self.program.scripts {
                                    if script.target == self.sprites[target].target
                                        && matches!(script.hat, Hat::Clone)
                                    {
                                        fibers.push(Fiber::new(
                                            next_id,
                                            target,
                                            script.entry,
                                            self.program.loop_slots,
                                        ));
                                        next_id += 1;
                                    }
                                }
                            }
                        }
                        Event::DeleteClone(target) => {
                            for fiber in &mut fibers {
                                if fiber.target == target {
                                    fiber.done = true;
                                }
                            }
                            self.delete_clone(target);
                        }
                    }
                }
                std::mem::swap(&mut self.current, &mut fibers[index]);
                self.publish();
                if let Some(error) = self.error.take() {
                    return Err(anyhow!(error));
                }
                index += 1;
                dispatched += 1;
            }
            fibers.retain(|f| !f.done);
            self.questions
                .retain(|q| fibers.iter().any(|f| f.ask_wait == Some(q.id)));
            self.publish_question();
            refresh_waiters(&mut fibers);
            // Host checkpoints in an uninterrupted warp block are not Scratch
            // redraw boundaries. Include broadcast receivers in the same round
            // so erase and render scripts reach the display together.
            let unfinished_warp = fibers.iter().any(|f| {
                f.warp()
                    && f.waiting.is_empty()
                    && f.wake.is_none()
                    && f.sound_wait.is_none()
                    && f.ask_wait.is_none()
            });
            if !unfinished_warp
                || pen_published.elapsed() >= Duration::from_millis(500)
                || self.pen.queued_bytes >= MAX_QUEUED_BYTES
            {
                self.pen
                    .publish_to(&mut self.shared.pen.lock().unwrap_or_else(|e| e.into_inner()));
                pen_published = Instant::now();
                // A slow/paused consumer cannot grow a persistent command queue
                // without bound. Clears replace obsolete frames; otherwise
                // apply backpressure while still honoring stop and time limits.
                while self
                    .shared
                    .pen
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .queued_bytes
                    >= MAX_QUEUED_BYTES
                    && !self.shared.stopped.load(Ordering::Relaxed)
                    && !self.limit_reached()
                {
                    thread::sleep(Duration::from_millis(1));
                }
            }
            let roots =
                self.program
                    .constants
                    .iter()
                    .copied()
                    .chain(self.vars.iter().copied())
                    .chain(self.lists.iter().flat_map(RuntimeList::roots))
                    .chain(self.clones.iter().flatten().flat_map(|c| {
                        c.vars
                            .iter()
                            .copied()
                            .chain(c.lists.iter().flat_map(RuntimeList::roots))
                    }))
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
            } else if fibers.iter().all(|f| {
                !f.waiting.is_empty()
                    || f.sound_wait.is_some()
                    || f.ask_wait.is_some()
                    || f.wake.is_some_and(|w| w > Instant::now())
            }) {
                let mut next = fibers
                    .iter()
                    .filter_map(|f| f.wake)
                    .min()
                    .unwrap_or(Instant::now() + Duration::from_millis(1));
                if fibers
                    .iter()
                    .any(|f| f.sound_wait.is_some() || f.ask_wait.is_some())
                {
                    next = next.min(Instant::now() + Duration::from_millis(1));
                }
                self.sleep_until(next);
            }
        }
        self.publish();
        self.publish_monitors();
        Ok(RunReport {
            steps: self.jit.steps,
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
            || self.options.max_steps.is_some_and(|s| self.jit.steps >= s)
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
    fn costume_pixels(&mut self, target: usize) -> Result<Option<Arc<CostumePixels>>> {
        let origin = self.sprites[target].target;
        let Some(costume) = self.project.targets[origin]
            .costumes
            .get(self.sprites[target].costume)
        else {
            return Ok(None);
        };
        #[cfg(feature = "graphics")]
        {
            let key = (origin, self.sprites[target].costume);
            if let Some(pixels) = self.costume_pixels.get(&key) {
                return Ok(Some(pixels.clone()));
            }
            let data = self
                .project
                .assets
                .get(&costume.md5ext)
                .context("costume asset is missing")?;
            let (w, h, rgba) = crate::render::decode(&costume.md5ext, data)?;
            if self.costume_bytes + rgba.len() > 64 * 1024 * 1024 {
                self.costume_pixels.clear();
                self.costume_bytes = 0;
            }
            self.costume_bytes += rgba.len();
            let mut pixels = CostumePixels::new(
                w,
                h,
                rgba,
                [costume.rotation_center_x, costume.rotation_center_y],
                costume.bitmap_resolution,
            );
            pixels.svg = costume.md5ext.to_ascii_lowercase().ends_with(".svg");
            let pixels = Arc::new(pixels);
            self.costume_pixels.insert(key, pixels.clone());
            Ok(Some(pixels))
        }
        #[cfg(not(feature = "graphics"))]
        {
            let _ = costume;
            anyhow::bail!(
                "costume stamps and edge sensing require the graphics feature (enabled by default)"
            )
        }
    }
    fn stamp(&mut self) -> Result<()> {
        if let Some(pixels) = self.costume_pixels(self.current.target)? {
            if self.pen.deferred {
                let mut stamp = PenCanvas::default();
                pixels.stamp(&self.sprites[self.current.target], &mut stamp);
                if !stamp.pixels.is_empty() {
                    self.pen.stamp_image(stamp.pixels);
                }
            } else {
                pixels.stamp(&self.sprites[self.current.target], &mut self.pen);
            }
        }
        Ok(())
    }
    fn touching_edge(&mut self) -> Result<bool> {
        Ok(self
            .costume_pixels(self.current.target)?
            .is_some_and(|p| p.touching_edge(&self.sprites[self.current.target])))
    }
    fn touching_object(&mut self, name: &str) -> Result<bool> {
        if name == "_edge_" {
            return self.touching_edge();
        }
        let target = self.current.target;
        if name == "_mouse_" {
            return Ok(self.costume_pixels(target)?.is_some_and(|pixels| {
                pixels.contains_point(&self.sprites[target], self.input.x, self.input.y)
            }));
        }
        let Some(origin) = self.named_sprite(name) else {
            return Ok(false);
        };
        if !self.sprites[target].visible {
            return Ok(false);
        }
        let Some(pixels) = self.costume_pixels(target)? else {
            return Ok(false);
        };
        for other in 0..self.sprites.len() {
            if other == target
                || !self.sprites[other].active
                || !self.sprites[other].visible
                || self.sprites[other].target != origin
            {
                continue;
            }
            if let Some(other_pixels) = self.costume_pixels(other)?
                && pixels.touching_sprite(
                    &self.sprites[target],
                    &other_pixels,
                    &self.sprites[other],
                )
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn play_sound(&mut self, value: &ScratchValue) -> Result<Option<u64>> {
        let target = self.current.target;
        let origin = self.sprites[target].target;
        let sounds = &self.project.targets[origin].sounds;
        let name = value.text();
        let index = sounds.iter().position(|s| s.name == name).or_else(|| {
            if sounds.is_empty() {
                return None;
            }
            let text = name.trim_start();
            let digits = text
                .strip_prefix(['+', '-'])
                .unwrap_or(text)
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
            if digits == 0 {
                return None;
            }
            let end = digits + usize::from(text.starts_with(['+', '-']));
            let n = text[..end].parse::<f64>().ok()?;
            n.is_finite()
                .then(|| (n - 1.0).rem_euclid(sounds.len() as f64) as usize)
        });
        if let Some(index) = index {
            return self
                .audio
                .play(
                    target,
                    (origin, index),
                    self.project
                        .assets
                        .get(&sounds[index].md5ext)
                        .context("sound asset is missing")?,
                    self.interactive,
                    self.sound_states[target],
                )
                .map(Some);
        }
        Ok(None)
    }
    fn fail(&mut self, message: &str) {
        self.error = Some(message.into());
        self.current.done = true;
        self.jit.budget = 0;
    }
}
// These functions have a C ABI and are only registered inside our JIT module.
// Their pointer argument is always a live, exclusively borrowed Vm supplied by
// NativeProgram::execute. None may unwind through generated machine code.
pub(crate) extern "C" fn rt_checkpoint(vm: &mut Vm, pc: u32) -> u32 {
    vm.current.pc = pc as usize;
    vm.jit.pc = pc;
    if vm.current.done || vm.error.is_some() {
        return 1;
    }
    if vm.shared.stopped.load(Ordering::Relaxed) || vm.limit_reached() {
        vm.shared.stopped.store(true, Ordering::Relaxed);
        vm.current.done = true;
        return 1;
    }
    let now = Instant::now();
    if vm.pen.queued_bytes >= MAX_QUEUED_BYTES {
        return 1;
    }
    if vm.current.warp() {
        let start = *vm.warp_start.get_or_insert(now);
        if now.duration_since(start) >= vm.options.warp_timeout {
            // Scratch cooperatively interrupts a long warp slice, preserving
            // its stack and PC. Expensive rendering is not a project error.
            return 1;
        }
    } else {
        vm.warp_start = None;
        if now.duration_since(vm.slice_start) >= Duration::from_millis(2) {
            return 1;
        }
    }
    vm.shared.steps.store(vm.jit.steps, Ordering::Relaxed);
    vm.jit.budget = vm
        .options
        .max_steps
        .map_or(64, |limit| (limit - vm.jit.steps).min(64) as u32);
    0
}
pub(crate) extern "C" fn rt_number(vm: &mut Vm, value: u64) -> f64 {
    vm.program.values.to_number(value)
}
pub(crate) extern "C" fn rt_bool(vm: &mut Vm, value: u64) -> u32 {
    u32::from(vm.program.values.to_bool(value))
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
    let (lists, views) = if vm.current.target >= vm.project.targets.len()
        && vm.program.list_names[slot].0 != vm.program.stage
    {
        let data = vm.clones[vm.current.target - vm.project.targets.len()]
            .as_mut()
            .unwrap();
        (&mut data.lists, &mut data.views)
    } else {
        (&mut vm.lists, &mut vm.list_views)
    };
    let list = &mut lists[slot];
    let values = &mut vm.program.values;
    let length = list.len();
    let result = match op {
        0 => {
            if length < LIMIT {
                list.push(a);
            }
            ScratchValue::Number(0.0)
        }
        1 => {
            if matches!(&arg, ScratchValue::String(s) if s == "all") {
                list.clear();
            } else if let Some(index) = list_index(&arg, length, &mut vm.random) {
                list.remove(index);
            }
            ScratchValue::Number(0.0)
        }
        2 => {
            list.clear();
            ScratchValue::Number(0.0)
        }
        3 => {
            if let Some(index) = list_index(&arg, length + 1, &mut vm.random).filter(|i| *i < LIMIT)
            {
                list.insert(index, b);
                if list.len() > LIMIT {
                    list.pop();
                }
            }
            ScratchValue::Number(0.0)
        }
        4 => {
            if let Some(index) = list_index(&arg, length, &mut vm.random) {
                list.replace(index, b);
            }
            ScratchValue::Number(0.0)
        }
        5 => {
            return list_index(&arg, length, &mut vm.random)
                .map(|i| list[i])
                .unwrap_or_else(|| values.encode(ScratchValue::String(String::new())));
        }
        6 => ScratchValue::Number(length as f64),
        7 => {
            let values: Vec<_> = list.iter().map(|v| values.decode(*v)).collect();
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
            let index = list
                .iter()
                .position(|v| values.decode(*v).compare(&arg).is_eq());
            if op == 9 {
                ScratchValue::Boolean(index.is_some())
            } else {
                ScratchValue::Number(index.map_or(0.0, |i| (i + 1) as f64))
            }
        }
        _ => ScratchValue::Number(0.0),
    };
    views[slot] = list.view();
    values.encode(result)
}
pub(crate) extern "C" fn rt_call_args(vm: &mut Vm, count: u32) -> *mut u64 {
    vm.call_args.resize(count as usize, 0);
    vm.call_args.as_mut_ptr()
}
pub(crate) extern "C" fn rt_call(vm: &mut Vm, procedure: u32, next: u32) -> u32 {
    if vm.current.frames.len() >= 1024 {
        vm.fail("custom block recursion exceeds 1024 frames");
        return 1;
    }
    let p = &vm.program.procedures[procedure as usize];
    let warp = p.warp || vm.current.warp();
    if warp != vm.current.warp() {
        vm.jit.budget = 0;
    }
    vm.current.pc = p.entry;
    vm.jit.pc = p.entry as u32;
    vm.jit.yield_on_loop = u32::from(!warp && !vm.options.turbo);
    let mut frame = vm.spare_frames.pop().unwrap_or(Frame {
        return_pc: 0,
        warp: false,
        args: vec![],
        loops: vec![],
    });
    frame.return_pc = next as usize;
    frame.warp = warp;
    frame.args.clear();
    frame.args.extend_from_slice(&vm.call_args);
    vm.call_args.clear();
    frame.loops.resize(vm.program.loop_slots, 0.0);
    frame.loops.fill(0.0);
    vm.jit.args = frame.args.as_ptr();
    vm.jit.loops = frame.loops.as_mut_ptr();
    vm.current.frames.push(frame);
    0
}
pub(crate) extern "C" fn rt_return(vm: &mut Vm) -> u32 {
    let mut frame = vm.current.frames.pop().unwrap();
    if vm.current.frames.is_empty() {
        vm.current.done = true;
        1
    } else {
        vm.current.pc = frame.return_pc;
        vm.jit.pc = frame.return_pc as u32;
        vm.jit.yield_on_loop = u32::from(!vm.current.warp() && !vm.options.turbo);
        if frame.warp != vm.current.warp() {
            vm.jit.budget = 0;
        }
        vm.jit.args = vm.current.frames.last().unwrap().args.as_ptr();
        vm.jit.loops = vm.current.frames.last_mut().unwrap().loops.as_mut_ptr();
        frame.args.clear();
        vm.spare_frames.push(frame);
        0
    }
}
pub(crate) extern "C" fn rt_wait(vm: &mut Vm, seconds: f64, next: u32) {
    let seconds = if seconds.is_nan() {
        0.0
    } else {
        seconds.clamp(0.0, 86400.0 * 365.0)
    };
    vm.current.wake = Some(Instant::now() + Duration::from_secs_f64(seconds));
    vm.current.pc = next as usize;
    vm.jit.pc = next;
}
pub(crate) extern "C" fn rt_glide(vm: &mut Vm, seconds: f64, x: f64, y: f64, next: u32) -> u32 {
    let target = vm.current.target;
    if seconds <= 0.0 || seconds.is_nan() {
        vm.move_to(target, x, y);
        return 0;
    }
    vm.current.glide = Some(Glide {
        start: Instant::now(),
        duration: seconds,
        from: (vm.sprites[target].x, vm.sprites[target].y),
        to: (x, y),
    });
    vm.current.wake = Some(Instant::now() + Duration::from_secs_f64(1.0 / vm.options.tick_hz));
    vm.jit.pc = next;
    vm.current.pc = next as usize;
    1
}
pub(crate) extern "C" fn rt_ask(vm: &mut Vm, question: u64, next: u32) {
    vm.next_question += 1;
    let prompt = QuestionPrompt {
        id: vm.next_question,
        text: vm.value(question).text(),
        draft: String::new(),
    };
    vm.current.ask_wait = Some(prompt.id);
    vm.questions.push_back(prompt);
    vm.current.pc = next as usize;
    vm.jit.pc = next;
    vm.publish_question();
}
pub(crate) extern "C" fn rt_play_until_done(vm: &mut Vm, sound: u64, next: u32) -> u32 {
    match vm.play_sound(&vm.value(sound)) {
        Ok(Some(sound)) => {
            vm.current.sound_wait = Some(sound);
            vm.current.pc = next as usize;
            vm.jit.pc = next;
            1
        }
        Ok(None) => 0,
        Err(error) => {
            vm.fail(&error.to_string());
            1
        }
    }
}
pub(crate) extern "C" fn rt_glide_to(vm: &mut Vm, seconds: f64, target: u64, next: u32) -> u32 {
    if let Some((x, y)) = vm.destination(&vm.value(target).text()) {
        rt_glide(vm, seconds, x, y, next)
    } else {
        0
    }
}
pub(crate) extern "C" fn rt_delete_clone(vm: &mut Vm) -> u32 {
    if vm.current.target < vm.project.targets.len() {
        return 0;
    }
    vm.current.done = true;
    vm.events.push(Event::DeleteClone(vm.current.target));
    1
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
    vm.jit.pc = next;
}
pub(crate) extern "C" fn rt_stop(vm: &mut Vm, mode: u32) {
    match mode {
        0 => {
            vm.events.push(Event::StopAll);
            if !vm.interactive {
                vm.shared.stopped.store(true, Ordering::Relaxed);
            }
            vm.current.done = true;
        }
        1 => {
            rt_return(vm);
        }
        _ => vm.events.push(Event::StopOthers),
    }
}
pub(crate) extern "C" fn rt_binary(vm: &mut Vm, op: u32, a: u64, b: u64) -> u64 {
    if op == 14 {
        return vm.property(&vm.value(a).text(), &vm.value(b).text());
    }
    if (5..=7).contains(&op) {
        let ordering = vm.program.values.compare(a, b);
        return vm.program.values.encode(ScratchValue::Boolean(match op {
            5 => ordering.is_lt(),
            6 => ordering.is_eq(),
            _ => ordering.is_gt(),
        }));
    }
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
    if (2..=16).contains(&op) {
        let n = vm.program.values.to_number(a);
        return number(match op {
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
            _ => unreachable!(),
        });
    }
    let a = vm.value(a);
    if op == 19 {
        let target = vm.current.target;
        let s = &vm.sprites[target];
        let name = a.text();
        let destination = if name == "_mouse_" {
            Some((vm.input.x, vm.input.y))
        } else {
            vm.project
                .targets
                .iter()
                .position(|t| !t.is_stage && t.name == name)
                .map(|i| (vm.sprites[i].x, vm.sprites[i].y))
        };
        return number(if s.is_stage {
            10000.0
        } else {
            destination
                .map(|(x, y)| (s.x - x).hypot(s.y - y))
                .unwrap_or(10000.0)
        });
    }
    if op == 18 {
        let result = vm.touching_object(&a.text());
        let touching = match result {
            Ok(value) => value,
            Err(error) => {
                vm.fail(&error.to_string());
                false
            }
        };
        return vm.program.values.encode(ScratchValue::Boolean(touching));
    }
    let result = match op {
        0 => ScratchValue::Boolean(!a.to_bool()),
        1 => ScratchValue::Number(a.text().chars().count() as f64),
        17 => ScratchValue::Boolean(vm.input.key_down(&a.text())),
        _ => ScratchValue::Number(0.0),
    };
    vm.program.values.encode(result)
}
pub(crate) extern "C" fn rt_state(vm: &mut Vm, field: u32) -> u64 {
    if field == 14 {
        return vm
            .program
            .values
            .encode(ScratchValue::String(vm.answer.clone()));
    }
    if field == 13 {
        return number(vm.sound_states[vm.current.target].volume);
    }
    if field == 12 {
        return vm
            .program
            .values
            .encode(ScratchValue::String(vm.options.language.clone()));
    }
    if field == 11 {
        return vm
            .program
            .values
            .encode(ScratchValue::String(vm.options.username.clone()));
    }
    if field == 10 {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        return number((seconds - 946_684_800.0) / 86_400.0);
    }
    if field == 9 {
        return vm
            .program
            .values
            .encode(ScratchValue::Boolean(vm.input.down));
    }
    let s = &vm.sprites[vm.current.target];
    if field == 6 {
        let name = vm.project.targets[s.target]
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
    if op == 42 || op == 43 {
        let name = value.text();
        if let Some((x, y)) = vm.destination(&name) {
            let target = vm.current.target;
            if op == 42 {
                vm.move_to(target, x, y);
            } else {
                let sprite = &mut vm.sprites[target];
                let direction = 90.0 - (y - sprite.y).atan2(x - sprite.x).to_degrees();
                sprite.direction = (direction + 180.0).rem_euclid(360.0) - 180.0;
                if sprite.direction == -180.0 {
                    sprite.direction = 180.0;
                }
                vm.publish_sprite(target);
            }
        }
        return;
    }
    if op == 44 {
        let n = vm.program.values.to_number(b);
        vm.relative_layer(if value.text() == "backward" { -n } else { n });
        return;
    }
    if (36..=41).contains(&op) {
        if op == 36 {
            vm.audio.stop();
            return;
        }
        let state = &mut vm.sound_states[vm.current.target];
        match op {
            37 => state.volume = value.to_number().clamp(0.0, 100.0),
            38 => state.volume = (state.volume + value.to_number()).clamp(0.0, 100.0),
            39 | 40 => state.effect(&value.text(), vm.program.values.to_number(b), op == 40),
            _ => {
                state.pitch = 0.0;
                state.pan = 0.0;
            }
        }
        vm.audio.configure(vm.current.target, *state);
        return;
    }
    if op == 33 {
        vm.create_clone(&value.text());
        return;
    }
    if op == 35 {
        vm.layer(vm.current.target, value.text() == "front");
        return;
    }
    if op == 28 || op == 29 {
        let slot = value.to_number() as usize;
        let mut monitors = vm.shared.monitors.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = monitors.iter_mut().find(|m| m.slot == slot) {
            m.visible = op == 28;
            if m.visible {
                m.update(vm.program.values.decode(vm.vars[slot]));
            }
        }
        return;
    }
    if op == 26 || op == 27 {
        let result = if op == 26 {
            vm.stamp()
        } else {
            vm.play_sound(&value).map(|_| ())
        };
        if let Err(error) = result {
            vm.fail(&error.to_string());
        }
        return;
    }
    let n = value.to_number();
    let m = vm.value(b).to_number();
    let target = vm.current.target;
    let s = &mut vm.sprites[target];
    let old_position = (s.x, s.y);
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
            let costumes = &vm.project.targets[s.target].costumes;
            if !costumes.is_empty() {
                if op == 15 {
                    s.costume = (s.costume + 1) % costumes.len();
                } else if !matches!(value, ScratchValue::Number(_))
                    && let Some(id) = costumes.iter().position(|c| c.name == value.text())
                {
                    s.costume = id;
                } else if value.text() == "next costume" {
                    s.costume = (s.costume + 1) % costumes.len();
                } else if value.text() == "previous costume" {
                    s.costume = (s.costume + costumes.len() - 1) % costumes.len();
                } else if vm.program.values.comparison_number(a).is_some() {
                    s.costume = ((n - 0.5).floor().rem_euclid(costumes.len() as f64)) as usize;
                }
            }
        }
        16 => vm.timer = Instant::now(),
        17 => s.rotation_style = value.text(),
        18 => vm.pen.clear(),
        19 => {
            let pen = &mut vm.pens[target];
            if !pen.down {
                pen.down = true;
                vm.pen.line(pen, old_position, old_position);
            }
        }
        20 => vm.pens[target].down = false,
        21 => vm.pens[target].size = n.clamp(1.0, 1200.0),
        22 => vm.pens[target].parameter(&value.text(), m, false),
        23 => vm.pens[target].parameter(&value.text(), m, true),
        24 => vm.pens[target].set_color(&value),
        25 => vm.pens[target].size = (vm.pens[target].size + n).clamp(1.0, 1200.0),
        30 | 31 => s.effects.update(&value.text(), m, op == 31),
        32 => s.effects = Default::default(),
        _ => {}
    }
    if matches!(op, 0..=5) && vm.pens[target].down && old_position != (s.x, s.y) {
        vm.pen.line(&vm.pens[target], old_position, (s.x, s.y));
    }
    if matches!(op, 6..=8) {
        s.direction = (s.direction + 180.0).rem_euclid(360.0) - 180.0;
        if s.direction == -180.0 {
            s.direction = 180.0;
        }
    }
    if matches!(op, 30..=32) {
        vm.publish_effects(target);
    } else {
        vm.publish_sprite(target);
    }
}

#[cfg(test)]
mod scheduler_tests {
    use super::*;

    #[test]
    fn warp_erase_and_redraw_survive_many_host_checkpoints() -> Result<()> {
        use crate::pen::PenCommand;
        let project = Arc::new(serde_json::from_value::<Project>(
            serde_json::json!({"targets":[{
                "name":"Stage", "isStage":true, "variables":{"n":["n",0]}, "blocks":{
                    "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"call"},
                    "call":{"opcode":"procedures_call","mutation":{"proccode":"draw","argumentids":"[]"}},
                    "def":{"opcode":"procedures_definition","topLevel":true,"inputs":{"custom_block":[1,"proto"]},"next":"erase"},
                    "proto":{"opcode":"procedures_prototype","mutation":{"proccode":"draw","argumentids":"[]","argumentnames":"[]","argumentdefaults":"[]","warp":"true"}},
                    "erase":{"opcode":"pen_clear","next":"repeat"},
                    "repeat":{"opcode":"control_repeat","inputs":{"TIMES":[1,[4,"50000"]],"SUBSTACK":[2,"inc"]},"next":"down"},
                    "inc":{"opcode":"data_changevariableby","fields":{"VARIABLE":["n","n"]},"inputs":{"VALUE":[1,[4,"1"]]}},
                    "down":{"opcode":"pen_penDown"}
                }
            }]}),
        )?);
        let running = spawn(
            project,
            RuntimeOptions {
                turbo: true,
                warp_timeout: Duration::from_micros(50),
                time_limit: Some(Duration::from_secs(10)),
                ..Default::default()
            },
        )?;
        let scene = running.scene.clone();
        scene.gpu_pen.store(true, Ordering::Release);
        while !running.is_finished() {
            let layer = scene.pen.lock().unwrap();
            if layer
                .commands
                .iter()
                .any(|c| matches!(c, PenCommand::Clear))
            {
                assert!(
                    layer
                        .commands
                        .iter()
                        .any(|c| matches!(c, PenCommand::Line(_))),
                    "a host checkpoint exposed erase before redraw"
                );
            }
            drop(layer);
            thread::sleep(Duration::from_micros(50));
        }
        let report = running.join()?;
        assert!(!report.interrupted);
        assert_eq!(report.variables[0].value, ScratchValue::Number(50000.));
        assert!(report.yields > 1, "fixture did not cross host checkpoints");
        assert_eq!(scene.pen.lock().unwrap().commands.len(), 2);
        Ok(())
    }

    #[test]
    fn finished_receivers_do_not_make_ready_callers_sleep() {
        let mut caller = Fiber::new(1, 0, 0, 0);
        caller.waiting.extend([2, 3]);
        let mut finished = Fiber::new(2, 0, 0, 0);
        finished.done = true;
        let restarted = Fiber::new(3, 0, 0, 0);
        let mut fibers = vec![caller, finished, restarted];
        refresh_waiters(&mut fibers);
        assert_eq!(fibers[0].waiting, HashSet::from([3]));
        fibers.pop();
        refresh_waiters(&mut fibers);
        assert!(fibers[0].waiting.is_empty());
    }
}
