// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureGate {
    DebugSession,
    Setup,
    Librarian,
}

pub struct HelpTopic {
    pub name: &'static str,
    pub one_liner: &'static str,
    pub body: &'static str,
    pub requires: Option<FeatureGate>,
}

pub static ALL_HELP_TOPICS: &[HelpTopic] = &[
    HelpTopic {
        name: "observations",
        one_liner: "querying, filtering, subscribing to runtime telemetry",
        body: include_str!("../tool_descriptions/help/observations.md"),
        requires: None,
    },
    HelpTopic {
        name: "envelope",
        one_liner: "the standard {result, daemon8, error} response shape",
        body: include_str!("../tool_descriptions/help/envelope.md"),
        requires: None,
    },
    HelpTopic {
        name: "checkpoint",
        one_liner: "bookmarking moments in the observation stream",
        body: include_str!("../tool_descriptions/help/checkpoint.md"),
        requires: None,
    },
    HelpTopic {
        name: "awareness",
        one_liner: "source, context, and reasoning awareness for a project",
        body: include_str!("../tool_descriptions/help/awareness.md"),
        requires: None,
    },
    HelpTopic {
        name: "lens",
        one_liner: "buffering matching observations between queries",
        body: include_str!("../tool_descriptions/help/lens.md"),
        requires: None,
    },
    HelpTopic {
        name: "debug_session",
        one_liner: "protocol for opening/closing debugging investigations",
        body: include_str!("../tool_descriptions/help/debug_session.md"),
        requires: Some(FeatureGate::DebugSession),
    },
    HelpTopic {
        name: "setup",
        one_liner: "first-time configuration and provider MCP registration",
        body: include_str!("../tool_descriptions/help/setup.md"),
        requires: Some(FeatureGate::Setup),
    },
    HelpTopic {
        name: "librarian",
        one_liner: "graph-based reference catalog for documentation, configs, fixes",
        body: include_str!("../tool_descriptions/help/librarian.md"),
        requires: Some(FeatureGate::Librarian),
    },
];

pub fn build_dynamic_index(enabled: &[FeatureGate], librarian_enabled: bool) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(1024);
    out.push_str("# daemon8 help\n\n");
    out.push_str(
        "Documentation is organized as isolated, AI-native context chunks. \
         Each topic is a self-contained unit — load only what you need for the current task \
         via `daemon8_help(topic=\"<name>\")`. This keeps your context window lean.\n\n",
    );
    out.push_str("## Topics\n\n");

    for topic in ALL_HELP_TOPICS {
        if topic_enabled(topic, enabled) {
            let _ = writeln!(out, "- `{}` — {}", topic.name, topic.one_liner);
        }
    }

    if librarian_enabled {
        out.push_str(
            "\n## Knowledge graph\n\n\
             The **librarian** extends this help system with a graph-based index of project-specific \
             references: documentation URLs, fix recipes, source configs, project protocols. \
             These are high-value pointers — the librarian never stores content, only locators to \
             where information lives. It complements whatever knowledge system you already use \
             (Obsidian, cloud storage, wikis) without replacing it.\n\n\
             Use `librarian_lookup` for project knowledge; `daemon8_help` for daemon8 protocol docs.\n",
        );
    }

    out.push_str("\nUnknown topics return this index.\n");
    out
}

pub fn find_topic(name: &str, enabled: &[FeatureGate]) -> Option<&'static HelpTopic> {
    ALL_HELP_TOPICS
        .iter()
        .find(|t| t.name == name && topic_enabled(t, enabled))
}

pub fn topic_enabled(topic: &HelpTopic, enabled: &[FeatureGate]) -> bool {
    match topic.requires {
        None => true,
        Some(gate) => enabled.contains(&gate),
    }
}
