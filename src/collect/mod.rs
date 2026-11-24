//! Collectors: read the system (through [`crate::sysroot::Sysroot`]) and
//! return raw facts. They never panic: anything unreadable becomes `None`.

pub mod host;
