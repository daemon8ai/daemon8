// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use owo_colors::OwoColorize;

pub fn blue(s: &str) -> String {
    s.truecolor(88, 166, 255).to_string()
}

pub fn green(s: &str) -> String {
    s.truecolor(126, 231, 135).to_string()
}

pub fn dim(s: &str) -> String {
    s.dimmed().to_string()
}

pub fn label(s: &str) -> String {
    format!("{:<14}", s).dimmed().to_string()
}
