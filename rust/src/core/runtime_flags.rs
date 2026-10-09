use std::sync::atomic::{AtomicBool, Ordering};

static QUIET: AtomicBool = AtomicBool::new(false);

pub struct FlagGuard {
    flag: &'static AtomicBool,
    previous: bool,
}

impl Drop for FlagGuard {
    fn drop(&mut self) {
        self.flag.store(self.previous, Ordering::Relaxed);
    }
}

fn set_scoped(flag: &'static AtomicBool) -> FlagGuard {
    let previous = flag.swap(true, Ordering::Relaxed);
    FlagGuard { flag, previous }
}

pub fn scoped_quiet() -> FlagGuard {
    set_scoped(&QUIET)
}

pub fn quiet_enabled() -> bool {
    QUIET.load(Ordering::Relaxed)
}
