//! Where the server finds the libraries it loads by name while it runs.

/// Restricts this process's search for libraries loaded by name, before anything loads one.
/// On Windows such a load (portable-pty's `conpty.dll`) otherwise also searches the current
/// directory and `PATH` when the executable's folder has no such file, so a repository
/// Workbench was started in could plant one; afterwards only the executable's folder and
/// System32 are searched. Nothing to do on Unix, whose dynamic linker never looks in the
/// working directory.
pub fn restrict_search() {
    sys::restrict_search()
}

#[cfg(unix)]
mod sys {
    pub fn restrict_search() {}
}

#[cfg(windows)]
mod sys {
    use windows_sys::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, SetDefaultDllDirectories};

    pub fn restrict_search() {
        // The executable's folder, System32 and AddDllDirectory folders (none). It fails only
        // for invalid flags, which these are not.
        // SAFETY: takes flags only; it changes this process's search path, which nothing has
        // used yet.
        unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS) };
    }
}
