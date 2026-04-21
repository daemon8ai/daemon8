// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub mod logcat;
pub mod loggingctl;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogSeverity {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct ParsedLine {
    pub timestamp: String,
    pub severity: LogSeverity,
    pub tag: String,
    pub pid: Option<u32>,
    pub message: String,
    pub hostname: Option<String>,
    pub facility: Option<String>,
}

pub trait LogParser: Send {
    fn parse_line(&self, line: &str) -> Option<ParsedLine>;
}
