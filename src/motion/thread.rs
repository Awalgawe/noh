//! The one thread that loads and evaluates a scene. The script interpreter
//! descends the stack at each expression level and call level, so how much
//! stack is left decides whether a legal script fits; a check made on one thread
//! proves nothing for another with less room (the window's paint callback runs
//! deep in the interface's stack). So every compilation and evaluation, in the
//! export worker as in the window, happens here: on a thread of its own, whose
//! stack has the same fixed size everywhere and starts empty at each request.
//! The caller hands over an instant and waits for the frame (a legal frame
//! costs a few milliseconds at worst); what comes back is host data.
use super::Inputs;
use super::audio::Audio;
use super::script::{Evaluated, ScriptScene};
use super::validate::{Report, validate};
use crate::fx::check::{Checker, Counts};
use crate::fx::draw::Block;
use crate::fx::text::Fonts;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;

/// The script thread's stack: at least twice the deepest legal evaluation,
/// measured in debug and release builds (see the test
/// `the_deepest_legal_script_fits_the_script_thread`).
pub const STACK: usize = 16 * 1024 * 1024;

type Reply<T> = mpsc::Sender<Result<T, String>>;

enum Request {
    Frame(f64, Reply<(Evaluated, Counts)>),
    Validate(Vec<f64>, Reply<Report>),
    Blocks(Reply<Vec<(Block, f32)>>),
}

pub struct ScriptThread {
    requests: Option<mpsc::Sender<Request>>,
    handle: Option<JoinHandle<()>>,
}

fn serve(
    src: String,
    inputs: Inputs,
    audio: Audio,
    count: bool,
    ready: Reply<()>,
    requests: mpsc::Receiver<Request>,
) {
    let fonts = match Fonts::load() {
        Ok(f) => Rc::new(RefCell::new(f)),
        Err(e) => {
            let _ = ready.send(Err(format!("fonts: {e}")));
            return;
        }
    };
    let scene = match ScriptScene::load(&src, &inputs, Rc::new(audio), fonts.clone(), count) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let mut checker = Checker::new(fonts.clone(), inputs.w, inputs.h);
    let _ = ready.send(Ok(()));
    for request in requests {
        match request {
            Request::Frame(t, reply) => {
                let r = scene
                    .frame(t)
                    .and_then(|f| f.check(&mut checker, t).map(|c| (f, c)));
                let _ = reply.send(r);
            }
            Request::Validate(instants, reply) => {
                let r = validate(
                    &scene,
                    &mut checker,
                    &fonts,
                    &instants,
                    inputs.safe,
                    (inputs.w, inputs.h),
                );
                let _ = reply.send(r);
            }
            Request::Blocks(reply) => {
                let _ = reply.send(Ok(checker.blocks()));
            }
        }
    }
}

impl ScriptThread {
    /// Load `src` against `inputs` on a new script thread. `count`: record the
    /// operations of each frame (for the validation report). A script that does
    /// not compile is refused here, with the reason.
    pub fn start(src: String, inputs: Inputs, audio: Audio, count: bool) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("motion-script".into())
            .stack_size(STACK)
            .spawn(move || serve(src, inputs, audio, count, ready_tx, rx))
            .map_err(|e| format!("script thread: {e}"))?;
        let this = Self {
            requests: Some(tx),
            handle: Some(handle),
        };
        ready_rx
            .recv()
            .map_err(|_| "script thread: ended while loading".to_string())??;
        Ok(this)
    }

    fn ask<T>(&self, make: impl FnOnce(Reply<T>) -> Request) -> Result<T, String> {
        let (tx, rx) = mpsc::channel();
        self.requests
            .as_ref()
            .ok_or("script thread: stopped")?
            .send(make(tx))
            .map_err(|_| "script thread: stopped".to_string())?;
        rx.recv().map_err(|_| "script thread: ended".to_string())?
    }

    /// The frame at `t`, checked: an error names the instant and the cap.
    pub fn frame(&self, t: f64) -> Result<(Evaluated, Counts), String> {
        self.ask(|r| Request::Frame(t, r))
    }

    /// The validation pass at `instants`.
    pub fn validate(&self, instants: Vec<f64>) -> Result<Report, String> {
        self.ask(|r| Request::Validate(instants, r))
    }

    /// Every block of text the frames checked so far laid out, each with the
    /// largest scale it reached: what a painter prepares after a validation.
    pub fn blocks(&self) -> Result<Vec<(Block, f32)>, String> {
        self.ask(Request::Blocks)
    }
}

impl Drop for ScriptThread {
    fn drop(&mut self) {
        self.requests.take();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
