// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

pub mod clf;
pub mod json;
pub mod line;
pub mod logfmt;
pub mod monolog;
pub mod syslog;

use crate::Parser;

pub fn get(name: &str) -> Option<Box<dyn Parser>> {
    match name {
        "monolog" => Some(Box::new(monolog::MonologParser)),
        "json" => Some(Box::new(json::JsonParser)),
        "line" => Some(Box::new(line::LineParser)),
        "syslog" => Some(Box::new(syslog::SyslogParser)),
        "logfmt" => Some(Box::new(logfmt::LogfmtParser)),
        "clf" => Some(Box::new(clf::ClfParser)),
        _ => None,
    }
}
