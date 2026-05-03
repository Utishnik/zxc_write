use std::cell::Cell;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crossfire::mpsc;

#[derive(Clone)]
pub enum LogTo {
    Ephemeral,
    File,
}