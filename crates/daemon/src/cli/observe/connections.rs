// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use anyhow::{Context, Result};
use daemon8_types::ConnectionInfo;

use super::{base_url, check_response, handle_reqwest_error, print_connections_table};

pub async fn cmd_connections(args: super::ClientArgs) -> Result<()> {
    let url = format!("{}/api/connections", base_url(args.resolved_port()));
    let resp = reqwest::get(&url)
        .await
        .map_err(|e| handle_reqwest_error(e, args.resolved_port()))?;
    let resp = check_response(resp).await?;

    if args.json {
        let raw: serde_json::Value = resp.json().await.unwrap_or_default();
        println!("{}", serde_json::to_string_pretty(&raw).unwrap_or_default());
        return Ok(());
    }

    #[derive(serde::Deserialize)]
    struct ConnectionsResponse {
        connections: Vec<ConnectionInfo>,
    }

    let data: ConnectionsResponse = resp
        .json()
        .await
        .context("failed to parse connections response")?;

    if data.connections.is_empty() {
        println!("No active connections.");
        return Ok(());
    }

    print_connections_table(&data.connections);
    Ok(())
}
