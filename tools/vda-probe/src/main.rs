// SPDX-License-Identifier: LicenseRef-FCL-1.0-ALv2
// Copyright (c) 2026 Havy.tech, LLC

use daemon8_adb::transport::AdbTransport;
use std::net::{Ipv4Addr, SocketAddrV4};

#[tokio::main]
async fn main() {
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 5037);
    println!("Connecting to ADB server at {addr}...");

    let transport = AdbTransport::new(addr);

    // Step 1: List devices
    println!("\n--- Device List ---");
    let devices = match transport.list_devices().await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to list devices: {e}");
            eprintln!("Is the ADB/VDA server running on port 5037?");
            return;
        }
    };

    if devices.is_empty() {
        println!("No devices connected.");
        println!("Connect a device and run: adb devices");
        return;
    }

    for d in &devices {
        println!("  {} | model: {} | state: {}", d.serial, d.model, d.state);
    }

    let serial = &devices[0].serial;

    // Step 2: Verify shell access
    println!("\n--- Shell Test (uname -a) ---");
    match transport.shell_command(serial, "uname -a").await {
        Ok(output) => println!("  {}", output.trim()),
        Err(e) => {
            eprintln!("Shell command failed: {e}");
            return;
        }
    }

    // Step 3: Stream 10 lines of log output
    // Try loggingctl first (Vega), fall back to logcat (Android)
    println!("\n--- Log stream (10 lines) ---");
    let cmd = match transport.shell_command(serial, "which loggingctl").await {
        Ok(output) if !output.trim().is_empty() && !output.contains("not found") => {
            println!("  (detected Vega OS -- using loggingctl)");
            "loggingctl log -o short_precise".to_string()
        }
        _ => {
            println!("  (using logcat)");
            "logcat -d -t 10".to_string()
        }
    };

    let (handle, mut rx, stop) = transport.spawn_log_stream(serial.clone(), cmd);

    let mut count = 0;
    while let Some(line) = rx.recv().await {
        count += 1;
        println!("  [{count:>2}] {line}");
        if count >= 10 {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            break;
        }
    }

    // Clean up
    drop(rx);
    let _ = handle.join();

    if count > 0 {
        println!("\nADB connectivity: OK ({count} lines captured)");
    } else {
        eprintln!("\nNo log output received");
    }
}
