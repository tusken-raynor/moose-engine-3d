//! Runs compiled scripts. Each placed script is an [instance](Runtime::spawn_instance) with
//! its own variables; each call of an async function (and each event handled) is a task
//! with its own stack, which stops at an `await` and carries on when the promise settles.
//!
//! Nothing is garbage collected: values are small copies, and the only things made as
//! scripts run are promises, kept in a table that is swept (by who still holds one) when it
//! has grown.

use std::collections::VecDeque;
use std::rc::Rc;

use crate::api::{Api, ApiDesc, ClassId, ClassKind, Host, HostFn, Type};
use crate::compile::{GlobalKind, Program};

/// A script value. What a handle is a handle of, the script's types say.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i32),
    Num(f32),
    /// A `Vec3` or a `Color`.
    Vec3([f32; 3]),
    /// A text, by its id in [`Strings`].
    Str(u32),
    /// One of the world's things, by the engine's index for it.
    Handle(u32),
    Promise(u32),
    /// What a `for...of` is walking.
    List(u32),
}

impl Value {
    /// As a number (0 for what isn't one).
    pub fn num(self) -> f32 {
        match self {
            Value::Num(n) => n,
            Value::Int(i) => i as f32,
            _ => 0.0,
        }
    }

    pub fn int(self) -> i32 {
        match self {
            Value::Int(i) => i,
            Value::Num(n) => n as i32,
            _ => 0,
        }
    }

    pub fn truth(self) -> bool {
        self == Value::Bool(true)
    }

    pub fn vec3(self) -> [f32; 3] {
        match self {
            Value::Vec3(v) => v,
            _ => [0.0; 3],
        }
    }

    /// A handle's index, unless this is `null`.
    pub fn handle(self) -> Option<usize> {
        match self {
            Value::Handle(i) => Some(i as usize),
            _ => None,
        }
    }
}

/// Every text scripts name, each once: a [`Value::Str`] is an index here.
#[derive(Debug, Default)]
pub struct Strings(Vec<String>);

impl Strings {
    pub fn intern(&mut self, text: &str) -> u32 {
        match self.0.iter().position(|s| s == text) {
            Some(i) => i as u32,
            None => {
                self.0.push(text.to_string());
                self.0.len() as u32 - 1
            }
        }
    }

    pub fn get(&self, id: u32) -> &str {
        self.0.get(id as usize).map_or("", String::as_str)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Op {
    Push(Value),
    Pop,
    Dup,
    GetLocal(u16),
    SetLocal(u16),
    GetGlobal(u16),
    SetGlobal(u16),
    /// The instance's `self`.
    SelfRef,
    IntToNum,
    /// Converts the value under the top one.
    IntToNumUnder,
    AddI,
    SubI,
    MulI,
    RemI,
    NegI,
    AddF,
    SubF,
    MulF,
    DivF,
    RemF,
    NegF,
    LtI,
    LeI,
    GtI,
    GeI,
    LtF,
    LeF,
    GtF,
    GeF,
    Eq,
    Ne,
    Not,
    IsNull,
    Jump(u32),
    /// Pops the condition.
    JumpIfFalse(u32),
    /// These two leave the value they test.
    JumpIfNull(u32),
    JumpIfNotNull(u32),
    /// A script's own function, and how many arguments are on the stack.
    Call(u32, u8),
    /// An async one: it runs as a new task, up to its first unsettled `await`, and its
    /// promise is left.
    Spawn(u32, u8),
    CallHost(u32, u8),
    Await,
    All(u8),
    Race(u8),
    /// `value!`: stops the task if it is null.
    NonNull,
    /// Steps a `for...of`: the next item into `item`, or on to `exit`.
    IterNext { list: u16, index: u16, item: u16, exit: u32 },
    ListFree(u16),
    Return,
}

/// A line scripts wrote to the log (`console.log`), or an error that stopped a task.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub error: bool,
}

enum Waiter {
    /// A task (and the generation of its slot) stopped at an `await`.
    Task(u32, u32),
    /// A `Promise.all` that this is one of.
    All(u32),
    Race(u32),
}

struct Slot {
    /// `None` while pending.
    value: Option<Value>,
    waiters: Vec<Waiter>,
    /// For a `Promise.all`: how many of its promises are still pending.
    remaining: u32,
    used: bool,
}

/// The promise table, and the tasks whose promises have settled since they stopped.
#[derive(Default)]
pub(crate) struct Promises {
    slots: Vec<Slot>,
    free: Vec<u32>,
    ready: VecDeque<(u32, u32, Value)>,
}

impl Promises {
    fn pending(&mut self) -> u32 {
        let slot = Slot { value: None, waiters: Vec::new(), remaining: 0, used: true };
        match self.free.pop() {
            Some(id) => {
                self.slots[id as usize] = slot;
                id
            }
            None => {
                self.slots.push(slot);
                self.slots.len() as u32 - 1
            }
        }
    }

    fn value(&self, id: u32) -> Option<Value> {
        self.slots.get(id as usize).filter(|s| s.used).and_then(|s| s.value)
    }

    /// Settles promise `id` (if it is pending), and what was waiting on it.
    fn resolve(&mut self, id: u32, value: Value) {
        let mut work = vec![(id, value)];
        while let Some((id, value)) = work.pop() {
            let Some(slot) = self.slots.get_mut(id as usize).filter(|s| s.used && s.value.is_none()) else {
                continue;
            };
            slot.value = Some(value);
            for waiter in std::mem::take(&mut slot.waiters) {
                match waiter {
                    Waiter::Task(task, generation) => self.ready.push_back((task, generation, value)),
                    Waiter::Race(race) => work.push((race, Value::Null)),
                    Waiter::All(all) => {
                        let all_slot = &mut self.slots[all as usize];
                        all_slot.remaining = all_slot.remaining.saturating_sub(1);
                        if all_slot.remaining == 0 {
                            work.push((all, Value::Null));
                        }
                    }
                }
            }
        }
    }
}

/// What a host function is handed besides the host: the script side of the call.
pub struct Cx<'a> {
    /// The thing the calling script is attached to (`self`), or `null`.
    pub owner: Value,
    /// Seconds scripts have run for, and the length of this tick.
    pub now: f64,
    pub delta: f32,
    strings: &'a mut Strings,
    promises: &'a mut Promises,
    timers: &'a mut Vec<(f64, u32)>,
    lists: &'a mut Lists,
    log: &'a mut Vec<Line>,
    rng: &'a mut u32,
}

impl Cx<'_> {
    /// The text of a string value.
    pub fn str(&self, value: Value) -> &str {
        match value {
            Value::Str(id) => self.strings.get(id),
            _ => "",
        }
    }

    pub fn string(&mut self, text: &str) -> Value {
        Value::Str(self.strings.intern(text))
    }

    /// A pending promise, for the host to [settle](Runtime::resolve) when what it stands
    /// for is done.
    pub fn promise(&mut self) -> Value {
        Value::Promise(self.promises.pending())
    }

    /// A promise that is already settled.
    pub fn settled(&mut self, value: Value) -> Value {
        let id = self.promises.pending();
        self.promises.resolve(id, value);
        Value::Promise(id)
    }

    /// Settles a promise the host made, from inside a call (see [`Runtime::resolve`]).
    pub fn resolve(&mut self, promise: Value, value: Value) {
        if let Value::Promise(id) = promise {
            self.promises.resolve(id, value);
        }
    }

    /// A promise that settles in `seconds`.
    pub fn sleep(&mut self, seconds: f32) -> Value {
        let id = self.promises.pending();
        self.timers.push((self.now + seconds.max(0.0) as f64, id));
        Value::Promise(id)
    }

    /// What a `for...of` walks: these values, in order.
    pub fn list(&mut self, items: impl IntoIterator<Item = Value>) -> Value {
        Value::List(self.lists.add(items))
    }

    pub fn log(&mut self, text: String) {
        self.log.push(Line { text, error: false });
    }

    /// A problem that doesn't stop the script (a call on something that is gone, say).
    pub fn warn(&mut self, text: String) {
        self.log.push(Line { text, error: true });
    }

    /// The next random number, from 0 up to (not including) 1.
    pub fn random(&mut self) -> f32 {
        // Xorshift: the same run of numbers every time, from the same seed.
        *self.rng ^= *self.rng << 13;
        *self.rng ^= *self.rng >> 17;
        *self.rng ^= *self.rng << 5;
        (*self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    /// A value as `console.log` shows it.
    pub fn show(&self, value: Value) -> String {
        match value {
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Num(n) => n.to_string(),
            Value::Vec3([x, y, z]) => format!("({x}, {y}, {z})"),
            Value::Str(id) => self.strings.get(id).to_string(),
            Value::Handle(i) => format!("#{i}"),
            Value::Promise(_) => "[promise]".to_string(),
            Value::List(_) => "[list]".to_string(),
        }
    }
}

/// The lists `for...of` loops are walking, reused as loops end.
#[derive(Default)]
struct Lists {
    lists: Vec<Vec<Value>>,
    free: Vec<u32>,
}

impl Lists {
    fn add(&mut self, items: impl IntoIterator<Item = Value>) -> u32 {
        let id = self.free.pop().unwrap_or_else(|| {
            self.lists.push(Vec::new());
            self.lists.len() as u32 - 1
        });
        self.lists[id as usize].extend(items);
        id
    }

    fn release(&mut self, id: u32) {
        if let Some(list) = self.lists.get_mut(id as usize) {
            list.clear();
            self.free.push(id);
        }
    }
}

struct Frame {
    func: u32,
    pc: u32,
    /// Where its locals start on the task's stack.
    base: u32,
}

struct Task {
    instance: u32,
    program: Rc<Program>,
    stack: Vec<Value>,
    frames: Vec<Frame>,
    /// The promise it settles when it returns.
    promise: u32,
    /// The lists its `for...of` loops hold.
    lists: Vec<u32>,
}

#[derive(Default)]
struct TaskSlot {
    /// `None` while it runs (it is taken out), or when the slot is free.
    task: Option<Task>,
    generation: u32,
    live: bool,
}

struct Instance {
    program: Rc<Program>,
    globals: Vec<Value>,
    owner: Value,
    /// What its inputs are bound to: it hears events about these (and its owner).
    watch: Vec<(ClassId, Value)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstanceId(u32);

enum Outcome {
    Waiting,
    Done(Value),
    Failed(String),
}

/// Operations a task may run before it must `await` or end: a loop that never does is
/// stopped, not left to hang the engine.
const BUDGET: u32 = 2_000_000;
/// How deep calls (and async calls starting async calls) may nest.
const MAX_DEPTH: usize = 200;

/// The scripts running in a world.
pub struct Runtime<K: Host> {
    fns: Vec<HostFn<K>>,
    desc: ApiDesc,
    pub strings: Strings,
    promises: Promises,
    timers: Vec<(f64, u32)>,
    lists: Lists,
    log: Vec<Line>,
    rng: u32,
    tasks: Vec<TaskSlot>,
    instances: Vec<Option<Instance>>,
    now: f64,
    delta: f32,
    /// How many promises are kept before the table is next swept.
    sweep_at: usize,
    depth: usize,
}

impl<K: Host> Runtime<K> {
    pub fn new(api: Api<K>) -> Runtime<K> {
        Runtime {
            fns: api.fns,
            desc: api.desc,
            strings: Strings::default(),
            promises: Promises::default(),
            timers: Vec::new(),
            lists: Lists::default(),
            log: Vec::new(),
            rng: 0x2545_f491,
            tasks: Vec::new(),
            instances: Vec::new(),
            now: 0.0,
            delta: 0.0,
            sweep_at: 64,
            depth: 0,
        }
    }

    pub fn desc(&self) -> &ApiDesc {
        &self.desc
    }

    /// Seconds scripts have run for.
    pub fn now(&self) -> f64 {
        self.now
    }

    /// Places a script: `inputs` are its inputs' values in [`Program::inputs`]' order, and
    /// `owner` what it is attached to (`self`), if anything. [`start`](Self::start) it next.
    pub fn spawn_instance(&mut self, program: Rc<Program>, owner: Value, inputs: &[Value]) -> InstanceId {
        let mut globals = vec![Value::Null; program.globals.len()];
        let mut watch = Vec::new();
        if let (Some(class), Value::Handle(_)) = (self.desc.self_class, owner) {
            watch.push((class, owner));
        }
        for ((slot, global), &value) in program.inputs().zip(inputs) {
            globals[slot] = value;
            let class = match &global.ty {
                Type::Class(c) => Some(*c),
                Type::Nullable(inner) => match **inner {
                    Type::Class(c) => Some(c),
                    _ => None,
                },
                _ => None,
            };
            if let (Some(class), Value::Handle(_)) = (class, value)
                && self.desc.classes[class as usize].kind == ClassKind::Handle
            {
                watch.push((class, value));
            }
        }
        let instance = Some(Instance { program, globals, owner, watch });
        let id = match self.instances.iter().position(Option::is_none) {
            Some(free) => {
                self.instances[free] = instance;
                free
            }
            None => {
                self.instances.push(instance);
                self.instances.len() - 1
            }
        };
        InstanceId(id as u32)
    }

    /// Sets an instance's constants and state to their starting values.
    pub fn start(&mut self, host: &mut K::Borrowed<'_>, id: InstanceId) {
        let Some(instance) = &self.instances[id.0 as usize] else {
            return;
        };
        let init = instance.program.init;
        self.run_function(host, id.0, init, &[]);
    }

    /// Removes an instance, and the tasks it had running.
    pub fn drop_instance(&mut self, id: InstanceId) {
        self.instances[id.0 as usize] = None;
        for i in 0..self.tasks.len() {
            if self.tasks[i].task.as_ref().is_some_and(|t| t.instance == id.0) {
                let task = self.tasks[i].task.take().unwrap();
                self.end_task(i as u32, task, Value::Null);
            }
        }
    }

    /// Removes every instance.
    pub fn clear(&mut self) {
        for i in 0..self.instances.len() {
            if self.instances[i].is_some() {
                self.drop_instance(InstanceId(i as u32));
            }
        }
    }

    /// An instance's state (its top-level `let`s) by name, to give to the instance that
    /// replaces it when its script changes.
    pub fn state(&self, id: InstanceId) -> Vec<(String, Type, Value)> {
        let Some(instance) = &self.instances[id.0 as usize] else {
            return Vec::new();
        };
        let globals = instance.program.globals.iter().zip(&instance.globals);
        globals
            .filter(|(g, _)| matches!(g.kind, GlobalKind::State { .. }) && !matches!(g.ty, Type::Promise(_)))
            .map(|(g, v)| (g.name.clone(), g.ty.clone(), *v))
            .collect()
    }

    /// Gives an instance the state of one it replaces: each variable it still has, by name
    /// and type.
    pub fn restore(&mut self, id: InstanceId, state: &[(String, Type, Value)]) {
        let Some(instance) = &mut self.instances[id.0 as usize] else {
            return;
        };
        for (global, slot) in instance.program.globals.iter().zip(&mut instance.globals) {
            if matches!(global.kind, GlobalKind::State { .. })
                && let Some((_, _, value)) = state.iter().find(|(n, t, _)| *n == global.name && *t == global.ty)
            {
                *slot = *value;
            }
        }
    }

    /// Tells scripts of event `event`: every instance that handles it, or with `about`,
    /// only those attached or bound to that thing.
    pub fn fire(&mut self, host: &mut K::Borrowed<'_>, event: u16, about: Option<(ClassId, Value)>, args: &[Value]) {
        for i in 0..self.instances.len() {
            let Some(instance) = &self.instances[i] else {
                continue;
            };
            let Some(&(_, func)) = instance.program.handlers.iter().find(|(e, _)| *e == event) else {
                continue;
            };
            if about.is_some_and(|about| !instance.watch.contains(&about)) {
                continue;
            }
            let takes = instance.program.functions[func as usize].params as usize;
            self.run_function(host, i as u32, func, &args[..takes.min(args.len())]);
        }
        self.run_ready(host);
    }

    /// Tells one instance of event `event`, if it handles it.
    pub fn fire_one(&mut self, host: &mut K::Borrowed<'_>, id: InstanceId, event: u16, args: &[Value]) {
        let handler = self.instances[id.0 as usize].as_ref().and_then(|i| i.program.handlers.iter().find(|(e, _)| *e == event).copied());
        if let Some((_, func)) = handler {
            self.run_function(host, id.0, func, args);
            self.run_ready(host);
        }
    }

    /// Moves scripts' time on by `dt` seconds: settles the `sleep`s that are over, and runs
    /// the tasks whose promises have settled.
    pub fn tick(&mut self, host: &mut K::Borrowed<'_>, dt: f32) {
        self.now += dt as f64;
        self.delta = dt;
        let mut i = 0;
        while i < self.timers.len() {
            if self.timers[i].0 <= self.now {
                let (_, promise) = self.timers.swap_remove(i);
                self.promises.resolve(promise, Value::Null);
            } else {
                i += 1;
            }
        }
        self.run_ready(host);
        if self.promises.slots.len() - self.promises.free.len() >= self.sweep_at {
            self.sweep();
        }
    }

    /// Settles a promise the host made (see [`Cx::promise`]). The tasks waiting on it run
    /// at the next [`tick`](Self::tick).
    pub fn resolve(&mut self, promise: Value, value: Value) {
        if let Value::Promise(id) = promise {
            self.promises.resolve(id, value);
        }
    }

    /// What scripts logged, and the errors that stopped tasks, since this was last asked.
    pub fn take_log(&mut self) -> Vec<Line> {
        std::mem::take(&mut self.log)
    }

    /// How many tasks are stopped at an `await`.
    pub fn waiting(&self) -> usize {
        self.tasks.iter().filter(|t| t.live).count()
    }

    /// How many promises the table holds.
    pub fn promises(&self) -> usize {
        self.promises.slots.len() - self.promises.free.len()
    }

    fn run_ready(&mut self, host: &mut K::Borrowed<'_>) {
        while let Some((id, generation, value)) = self.promises.ready.pop_front() {
            let slot = &mut self.tasks[id as usize];
            if slot.generation != generation {
                continue;
            }
            if let Some(mut task) = slot.task.take() {
                task.stack.push(value);
                self.run_task(host, id, task);
            }
        }
    }

    /// Starts function `func` of an instance as a task, runs it as far as it goes, and
    /// returns its promise.
    fn run_function(&mut self, host: &mut K::Borrowed<'_>, instance: u32, func: u32, args: &[Value]) -> u32 {
        let promise = self.promises.pending();
        let Some(program) = self.instances[instance as usize].as_ref().map(|i| Rc::clone(&i.program)) else {
            self.promises.resolve(promise, Value::Null);
            return promise;
        };
        let function = &program.functions[func as usize];
        let mut stack = args.to_vec();
        stack.resize(function.locals as usize, Value::Null);
        let task = Task {
            instance,
            program,
            stack,
            frames: vec![Frame { func, pc: 0, base: 0 }],
            promise,
            lists: Vec::new(),
        };
        let id = match self.tasks.iter().position(|t| !t.live) {
            Some(free) => free,
            None => {
                self.tasks.push(TaskSlot::default());
                self.tasks.len() - 1
            }
        };
        self.tasks[id].live = true;
        self.run_task(host, id as u32, task);
        promise
    }

    fn run_task(&mut self, host: &mut K::Borrowed<'_>, id: u32, mut task: Task) {
        self.depth += 1;
        let outcome = if self.depth > MAX_DEPTH {
            Outcome::Failed("async calls are nested too deep".to_string())
        } else {
            self.exec(host, id, &mut task)
        };
        self.depth -= 1;
        match outcome {
            Outcome::Waiting => self.tasks[id as usize].task = Some(task),
            Outcome::Done(value) => self.end_task(id, task, value),
            Outcome::Failed(message) => {
                let at = task.frames.last().map_or(String::new(), |frame| {
                    let function = &task.program.functions[frame.func as usize];
                    let line = function.lines.get(frame.pc.saturating_sub(1) as usize).copied().unwrap_or(0);
                    format!("{}:{line}: ", task.program.files[function.file as usize])
                });
                self.log.push(Line { text: format!("{at}{message}"), error: true });
                self.end_task(id, task, Value::Null);
            }
        }
    }

    /// Frees a task's slot and lists, and settles its promise.
    fn end_task(&mut self, id: u32, task: Task, value: Value) {
        for list in task.lists {
            self.lists.release(list);
        }
        let slot = &mut self.tasks[id as usize];
        slot.live = false;
        slot.generation += 1;
        self.promises.resolve(task.promise, value);
    }

    /// Runs `task` until it ends or stops at an `await`.
    fn exec(&mut self, host: &mut K::Borrowed<'_>, id: u32, task: &mut Task) -> Outcome {
        let program = Rc::clone(&task.program);
        let mut budget = BUDGET;
        loop {
            let frame = task.frames.last_mut().unwrap();
            let function = &program.functions[frame.func as usize];
            let op = function.code[frame.pc as usize];
            frame.pc += 1;
            let base = frame.base as usize;
            budget -= 1;
            if budget == 0 {
                return Outcome::Failed("ran too long without an await (a loop that never ends?)".to_string());
            }
            // The value on top of the stack, and the two on top, taken off.
            macro_rules! pop {
                () => {
                    task.stack.pop().unwrap_or(Value::Null)
                };
            }
            macro_rules! pop2 {
                () => {{
                    let b = pop!();
                    (pop!(), b)
                }};
            }
            match op {
                Op::Push(value) => task.stack.push(value),
                Op::Pop => {
                    task.stack.pop();
                }
                Op::Dup => task.stack.push(task.stack.last().copied().unwrap_or(Value::Null)),
                Op::GetLocal(slot) => task.stack.push(task.stack[base + slot as usize]),
                Op::SetLocal(slot) => task.stack[base + slot as usize] = pop!(),
                Op::GetGlobal(slot) => {
                    let instance = self.instances[task.instance as usize].as_ref();
                    task.stack.push(instance.map_or(Value::Null, |i| i.globals[slot as usize]));
                }
                Op::SetGlobal(slot) => {
                    let value = pop!();
                    if let Some(instance) = &mut self.instances[task.instance as usize] {
                        instance.globals[slot as usize] = value;
                    }
                }
                Op::SelfRef => {
                    let instance = self.instances[task.instance as usize].as_ref();
                    task.stack.push(instance.map_or(Value::Null, |i| i.owner));
                }
                Op::IntToNum => {
                    let value = pop!();
                    task.stack.push(if value == Value::Null { value } else { Value::Num(value.num()) });
                }
                Op::IntToNumUnder => {
                    let at = task.stack.len().saturating_sub(2);
                    task.stack[at] = Value::Num(task.stack[at].num());
                }
                Op::AddI | Op::SubI | Op::MulI | Op::RemI => {
                    let (a, b) = pop2!();
                    let (a, b) = (a.int(), b.int());
                    task.stack.push(Value::Int(match op {
                        Op::AddI => a.wrapping_add(b),
                        Op::SubI => a.wrapping_sub(b),
                        Op::MulI => a.wrapping_mul(b),
                        _ if b == 0 => return Outcome::Failed("the remainder of dividing by 0".to_string()),
                        _ => a.wrapping_rem(b),
                    }));
                }
                Op::NegI => {
                    let a = pop!();
                    task.stack.push(Value::Int(a.int().wrapping_neg()));
                }
                Op::AddF | Op::SubF | Op::MulF | Op::DivF | Op::RemF => {
                    let (a, b) = pop2!();
                    let (a, b) = (a.num(), b.num());
                    task.stack.push(Value::Num(match op {
                        Op::AddF => a + b,
                        Op::SubF => a - b,
                        Op::MulF => a * b,
                        Op::DivF => a / b,
                        _ => a % b,
                    }));
                }
                Op::NegF => {
                    let a = pop!();
                    task.stack.push(Value::Num(-a.num()));
                }
                Op::LtI | Op::LeI | Op::GtI | Op::GeI => {
                    let (a, b) = pop2!();
                    let (a, b) = (a.int(), b.int());
                    task.stack.push(Value::Bool(match op {
                        Op::LtI => a < b,
                        Op::LeI => a <= b,
                        Op::GtI => a > b,
                        _ => a >= b,
                    }));
                }
                Op::LtF | Op::LeF | Op::GtF | Op::GeF => {
                    let (a, b) = pop2!();
                    let (a, b) = (a.num(), b.num());
                    task.stack.push(Value::Bool(match op {
                        Op::LtF => a < b,
                        Op::LeF => a <= b,
                        Op::GtF => a > b,
                        _ => a >= b,
                    }));
                }
                Op::Eq | Op::Ne => {
                    let (a, b) = pop2!();
                    task.stack.push(Value::Bool((a == b) == matches!(op, Op::Eq)));
                }
                Op::Not => {
                    let a = pop!();
                    task.stack.push(Value::Bool(!a.truth()));
                }
                Op::IsNull => {
                    let a = pop!();
                    task.stack.push(Value::Bool(a == Value::Null));
                }
                Op::Jump(to) => task.frames.last_mut().unwrap().pc = to,
                Op::JumpIfFalse(to) => {
                    if !pop!().truth() {
                        task.frames.last_mut().unwrap().pc = to;
                    }
                }
                Op::JumpIfNull(to) | Op::JumpIfNotNull(to) => {
                    let null = task.stack.last().is_none_or(|v| *v == Value::Null);
                    if null == matches!(op, Op::JumpIfNull(_)) {
                        task.frames.last_mut().unwrap().pc = to;
                    }
                }
                Op::Call(func, argc) => {
                    if task.frames.len() >= MAX_DEPTH {
                        return Outcome::Failed("calls are nested too deep (a function that calls itself forever?)".to_string());
                    }
                    let base = task.stack.len() - argc as usize;
                    task.stack.resize(base + program.functions[func as usize].locals as usize, Value::Null);
                    task.frames.push(Frame { func, pc: 0, base: base as u32 });
                }
                Op::Spawn(func, argc) => {
                    let at = task.stack.len() - argc as usize;
                    let args: Vec<Value> = task.stack.drain(at..).collect();
                    let promise = self.run_function(host, task.instance, func, &args);
                    task.stack.push(Value::Promise(promise));
                }
                Op::CallHost(func, argc) => {
                    let at = task.stack.len() - argc as usize;
                    let owner = self.instances[task.instance as usize].as_ref().map_or(Value::Null, |i| i.owner);
                    let mut cx = Cx {
                        owner,
                        now: self.now,
                        delta: self.delta,
                        strings: &mut self.strings,
                        promises: &mut self.promises,
                        timers: &mut self.timers,
                        lists: &mut self.lists,
                        log: &mut self.log,
                        rng: &mut self.rng,
                    };
                    let result = (self.fns[func as usize])(host, &mut cx, &task.stack[at..]);
                    task.stack.truncate(at);
                    if let Value::List(list) = result {
                        task.lists.push(list);
                    }
                    task.stack.push(result);
                }
                Op::Await => {
                    let Value::Promise(promise) = pop!() else {
                        task.stack.push(Value::Null);
                        continue;
                    };
                    match self.promises.value(promise) {
                        Some(value) => task.stack.push(value),
                        None => {
                            let generation = self.tasks[id as usize].generation;
                            self.promises.slots[promise as usize].waiters.push(Waiter::Task(id, generation));
                            return Outcome::Waiting;
                        }
                    }
                }
                Op::All(n) | Op::Race(n) => {
                    let at = task.stack.len() - n as usize;
                    let parts: Vec<Value> = task.stack.drain(at..).collect();
                    let race = matches!(op, Op::Race(_));
                    let whole = self.promises.pending();
                    let mut pending = 0;
                    for part in &parts {
                        if let Value::Promise(part) = *part
                            && self.promises.value(part).is_none()
                        {
                            pending += 1;
                            let waiter = if race { Waiter::Race(whole) } else { Waiter::All(whole) };
                            self.promises.slots[part as usize].waiters.push(waiter);
                        }
                    }
                    self.promises.slots[whole as usize].remaining = pending;
                    // All settled already (or, for a race, any).
                    if pending == 0 || (race && (pending as usize) < parts.len()) {
                        self.promises.resolve(whole, Value::Null);
                    }
                    task.stack.push(Value::Promise(whole));
                }
                Op::NonNull => {
                    if task.stack.last().is_none_or(|v| *v == Value::Null) {
                        return Outcome::Failed("a value marked '!' was null".to_string());
                    }
                }
                Op::IterNext { list, index, item, exit } => {
                    let at = task.stack[base + index as usize].int();
                    let next = match task.stack[base + list as usize] {
                        Value::List(l) => self.lists.lists[l as usize].get(at as usize).copied(),
                        _ => None,
                    };
                    match next {
                        Some(value) => {
                            task.stack[base + item as usize] = value;
                            task.stack[base + index as usize] = Value::Int(at + 1);
                        }
                        None => task.frames.last_mut().unwrap().pc = exit,
                    }
                }
                Op::ListFree(slot) => {
                    if let Value::List(list) = task.stack[base + slot as usize] {
                        task.lists.retain(|l| *l != list);
                        self.lists.release(list);
                        task.stack[base + slot as usize] = Value::Null;
                    }
                }
                Op::Return => {
                    let value = pop!();
                    task.stack.truncate(base);
                    task.frames.pop();
                    if task.frames.is_empty() {
                        return Outcome::Done(value);
                    }
                    task.stack.push(value);
                }
            }
        }
    }

    /// Frees the settled promises nothing holds any more: not a task's stack, an
    /// instance's variables, nor a `Promise.all` or `Promise.race` still waiting.
    fn sweep(&mut self) {
        let mut held = vec![false; self.promises.slots.len()];
        let mut hold = |value: &Value| {
            if let Value::Promise(id) = value
                && let Some(h) = held.get_mut(*id as usize)
            {
                *h = true;
            }
        };
        for task in self.tasks.iter().filter_map(|t| t.task.as_ref()) {
            task.stack.iter().for_each(&mut hold);
            hold(&Value::Promise(task.promise));
        }
        for instance in self.instances.iter().flatten() {
            instance.globals.iter().for_each(&mut hold);
        }
        self.promises.ready.iter().for_each(|(_, _, value)| hold(value));
        for slot in &self.promises.slots {
            for waiter in &slot.waiters {
                if let Waiter::All(id) | Waiter::Race(id) = waiter {
                    hold(&Value::Promise(*id));
                }
            }
        }
        for (id, slot) in self.promises.slots.iter_mut().enumerate() {
            if slot.used && slot.value.is_some() && !held[id] {
                slot.used = false;
                self.promises.free.push(id as u32);
            }
        }
        self.sweep_at = ((self.promises.slots.len() - self.promises.free.len()) * 2).max(64);
    }
}
