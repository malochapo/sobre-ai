pub const DEFAULT_MAX_READ_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_SHELL_BYTES: usize = 2 * 1024 * 1024;

#[cfg(test)]
thread_local! {
    static READ_CAP_FOR_THIS_THREAD: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// Test-only: cap reads made on the calling thread.
#[cfg(test)]
pub(crate) fn set_max_read_bytes_for_this_thread(cap: Option<usize>) {
    READ_CAP_FOR_THIS_THREAD.with(|c| c.set(cap));
}

pub fn max_read_bytes() -> usize {
    #[cfg(test)]
    if let Some(cap) = READ_CAP_FOR_THIS_THREAD.with(std::cell::Cell::get) {
        return cap.max(1024);
    }
    DEFAULT_MAX_READ_BYTES
}

pub fn max_shell_bytes() -> usize {
    DEFAULT_MAX_SHELL_BYTES
}
