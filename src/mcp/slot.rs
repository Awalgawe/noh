//! One heavy job at a time per server (agent-follow note §5, maintainer's
//! Q3): a job of the headless tools, or a job a driven window runs for the
//! agent. Taken before the work is asked for, so two cannot start together.
use std::sync::{Arc, Mutex};

/// Who holds the slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holder {
    /// A job of this session's headless tools, by its job id.
    Headless(String),
    /// A driven window, by its app id: reserved while its gesture is
    /// forwarded, held once the window says the job started.
    Window(String),
}

impl Holder {
    /// Why something else waits, in words.
    pub fn describe(&self) -> String {
        match self {
            Holder::Headless(id) => format!("the headless job {id} of this session is running"),
            Holder::Window(id) => format!("the NOH window {id} is running a job"),
        }
    }
}

/// The server's slot, shared by the session and its windows.
#[derive(Clone, Default)]
pub struct Slot(Arc<Mutex<Option<Holder>>>);

impl Slot {
    /// Take the slot for `holder`; the current holder if it is taken.
    pub fn take(&self, holder: Holder) -> Result<(), Holder> {
        let mut slot = self.0.lock().unwrap();
        match &*slot {
            Some(current) if *current != holder => Err(current.clone()),
            _ => {
                *slot = Some(holder);
                Ok(())
            }
        }
    }

    /// Give the slot back, if `holder` holds it.
    pub fn release(&self, holder: &Holder) {
        let mut slot = self.0.lock().unwrap();
        if slot.as_ref() == Some(holder) {
            *slot = None;
        }
    }

    pub fn holder(&self) -> Option<Holder> {
        self.0.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time_and_only_it_gives_back() {
        let slot = Slot::default();
        let window = Holder::Window("app-1".into());
        let job = Holder::Headless("job-1".into());
        assert!(slot.take(window.clone()).is_ok());
        assert_eq!(slot.take(job.clone()), Err(window.clone()));
        // Another holder cannot give it back.
        slot.release(&job);
        assert_eq!(slot.holder(), Some(window.clone()));
        slot.release(&window);
        assert!(slot.take(job.clone()).is_ok());
        assert!(slot.holder().unwrap().describe().contains("job-1"));
    }
}
