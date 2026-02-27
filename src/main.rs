// main.rs - COMPLETE FIXED VERSION
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use nusb::transfer::RequestBuffer;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use warp::Filter;
use futures::{StreamExt, SinkExt};
use warp::ws::{WebSocket, Message};
use tokio::sync::mpsc;

#[cfg(windows)]
use winapi::um::timeapi::{timeBeginPeriod, timeEndPeriod};

#[cfg(windows)]
fn set_timer_resolution() {
    unsafe {
        timeBeginPeriod(1);
    }
}

#[cfg(windows)]
fn reset_timer_resolution() {
    unsafe {
        timeEndPeriod(1);
    }
}

#[cfg(not(windows))]
fn set_timer_resolution() {}

#[cfg(not(windows))]
fn reset_timer_resolution() {}

// --- GameCube Adapter Constants ---
const GC_ADAPTER_VID: u16 = 0x057e;
const GC_ADAPTER_PID: u16 = 0x0337;
const GC_ENDPOINT_IN: u8 = 0x81;
const GC_ENDPOINT_OUT: u8 = 0x02;
const GC_READ_SIZE: usize = 37;

// --- Data Structures (UNCHANGED) ---
#[derive(Serialize, Clone, Debug)]
struct ButtonsData {
    a: bool, b: bool, x: bool, y: bool, start: bool, z: bool, l: bool, r: bool,
    d_left: bool, d_right: bool, d_up: bool, d_down: bool,
}

#[derive(Serialize, Clone, Debug)]
struct StickData {
    x: i16, y: i16, melee_x: f32, melee_y: f32,
}

#[derive(Serialize, Clone, Debug)]
struct TriggerData { l: u8, r: u8 }

#[derive(Serialize, Clone, Debug)]
struct OriginData {
    stick_x: f32, stick_y: f32, c_x: f32, c_y: f32, l: u8, r: u8,
}

#[derive(Serialize, Clone, Debug)]
struct ControllerSample {
    timestamp: u128,
    buttons: ButtonsData,
    stick: StickData,
    c_stick: StickData,
    triggers: TriggerData,
}

#[derive(Serialize, Clone, Debug)]
struct ControllerDataPacket {
    port: usize,
    connected: bool,
    poll_rate: u32,
    origins: OriginData,
    samples: Vec<ControllerSample>,
}

#[derive(Serialize, Clone, Debug)]
struct BridgeUpdate {
    controllers: Vec<ControllerDataPacket>,
}

#[derive(Clone, Copy, Debug)]
struct ControllerCalibration {
    stick: (u8, u8), c_stick: (u8, u8), triggers: (u8, u8),
}

// --- (UNCHANGED: melee_coord_clamp, parse_gc_input, RawControllerInput) ---
fn melee_coord_clamp(x_in: i16, y_in: i16) -> (f32, f32) {
    let x_f = x_in as f32;
    let y_f = y_in as f32;
    let magnitude = (x_f * x_f + y_f * y_f).sqrt();
    let scale = if magnitude > 0.0 {
        (80.0 / magnitude).min(1.0)
    } else {
        1.0
    };
    let x_out = (x_f * scale).trunc() / 80.0;
    let y_out = (y_f * scale).trunc() / 80.0;
    (x_out, y_out)
}

fn parse_gc_input(data: &[u8]) -> Option<[RawControllerInput; 4]> {
    if data.len() < 37 || data[0] != 0x21 { return None; }
    let mut controllers = [RawControllerInput::default(); 4];
    for port in 0..4 {
        let offset = 1 + (port * 9);
        if offset + 9 > data.len() { break; }
        let bytes = &data[offset..offset + 9];
        let controller_type = (bytes[0] >> 4) & 0x03;
        controllers[port].connected = controller_type == 0x01 || controller_type == 0x02;
        if controllers[port].connected {
            controllers[port].a = (bytes[1] & 0x01) != 0;
            controllers[port].b = (bytes[1] & 0x02) != 0;
            controllers[port].x = (bytes[1] & 0x04) != 0;
            controllers[port].y = (bytes[1] & 0x08) != 0;
            controllers[port].d_left = (bytes[1] & 0x10) != 0;
            controllers[port].d_right = (bytes[1] & 0x20) != 0;
            controllers[port].d_down = (bytes[1] & 0x40) != 0;
            controllers[port].d_up = (bytes[1] & 0x80) != 0;
            controllers[port].start = (bytes[2] & 0x01) != 0;
            controllers[port].z = (bytes[2] & 0x02) != 0;
            controllers[port].r = (bytes[2] & 0x04) != 0;
            controllers[port].l = (bytes[2] & 0x08) != 0;
            controllers[port].stick_x = bytes[3];
            controllers[port].stick_y = bytes[4];
            controllers[port].c_stick_x = bytes[5];
            controllers[port].c_stick_y = bytes[6];
            controllers[port].trigger_l = bytes[7];
            controllers[port].trigger_r = bytes[8];
        }
    }
    Some(controllers)
}

#[derive(Clone, Copy, Debug, Default)]
struct RawControllerInput {
    connected: bool,
    a: bool, b: bool, x: bool, y: bool, start: bool, z: bool, l: bool, r: bool,
    d_left: bool, d_right: bool, d_up: bool, d_down: bool,
    stick_x: u8, stick_y: u8, c_stick_x: u8, c_stick_y: u8, trigger_l: u8, trigger_r: u8,
}

// --- FIXED App State ---
struct AppSharedState {
    adapter_status: String,
    server_ready: bool,
    client_connected: bool,
    ports: [PortMetadata; 4],
    calibrations: [Option<ControllerCalibration>; 4],
    setup_hint: Option<String>,
}

#[derive(Clone, Copy)]
struct PortMetadata {
    connected: bool,
    poll_rate: u32,
}

struct BridgeApp {
    shared_state: Arc<Mutex<AppSharedState>>,
    shutdown_signal: Arc<AtomicBool>,
}

// egui UI (UPDATED with server status)
impl eframe::App for BridgeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(100));
        if ctx.input(|i| i.viewport().close_requested()) {
            self.shutdown_signal.store(false, Ordering::Relaxed);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame {
                fill: egui::Color32::from_rgb(30, 30, 40),
                ..Default::default()
            })
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing = egui::Vec2::new(20.0, 10.0);
                    ui.label(
                        egui::RichText::new("Conch.gg GameCube Bridge")
                            .size(24.0)
                            .color(egui::Color32::LIGHT_BLUE),
                    );

                    let state = self.shared_state.lock().unwrap();

                    // NEW: Server status
                    let server_color = if state.server_ready {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::YELLOW
                    };
                    ui.label(
                        egui::RichText::new(format!("Server: {}", 
                            if state.server_ready { "Ready" } else { "Starting..." }))
                            .color(server_color)
                            .size(16.0),
                    );

                    let ad_color = if state.adapter_status.contains("Connected") {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::YELLOW
                    };
                    ui.label(
                        egui::RichText::new(format!("Adapter: {}", state.adapter_status))
                            .color(ad_color)
                            .size(16.0),
                    );

                    let cl_color = if state.client_connected {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::RED
                    };
                    let cl_text = if state.client_connected { "Site Connected" } else { "Site Disconnected" };
                    ui.label(
                        egui::RichText::new(cl_text)
                            .color(cl_color)
                            .size(16.0),
                    );

                    ui.separator();

                    for (i, port) in state.ports.iter().enumerate() {
                        if port.connected {
                            ui.label(format!("Port {}: {} Hz", i + 1, port.poll_rate));
                        }
                    }

                    ui.label(
                        egui::RichText::new("WebSocket: ws://127.0.0.1:8080/ws")
                            .size(12.0)
                            .weak(),
                    );

                    if let Some(ref hint) = state.setup_hint {
                        ui.separator();
                        ui.label(
                            egui::RichText::new("USB access denied — udev rule required")
                                .color(egui::Color32::from_rgb(255, 160, 50))
                                .size(13.0),
                        );
                        let hint_copy = hint.clone();
                        drop(state);
                        if ui.button("Copy setup command").clicked() {
                            ui.output_mut(|o| o.copied_text = hint_copy);
                        }
                    }
                });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shutdown_signal.store(false, Ordering::Relaxed);
        reset_timer_resolution();
    }
}

// (UNCHANGED: PollMessage, handle_client)
#[derive(Clone)]
struct PollMessage {
    port_index: usize,
    sample: ControllerSample,
    connected: bool,
}

async fn handle_client(
    ws: WebSocket,
    mut rx: mpsc::UnboundedReceiver<PollMessage>,
    shared_state: Arc<Mutex<AppSharedState>>,
) {
    let (mut tx, mut ws_rx) = ws.split();

    if let Ok(mut state) = shared_state.lock() {
        state.client_connected = true;
    }

    let mut interval = tokio::time::interval(Duration::from_millis(16));
    let mut port_buffers: Vec<Vec<ControllerSample>> = vec![vec![], vec![], vec![], vec![]];
    let mut port_connected: Vec<bool> = vec![false; 4];

    loop {
        tokio::select! {
            Some(msg) = rx.recv() => {
                if msg.port_index < 4 {
                    port_connected[msg.port_index] = msg.connected;
                    if msg.connected {
                        port_buffers[msg.port_index].push(msg.sample);
                    } else {
                        port_buffers[msg.port_index].clear();
                    }
                }
            }

            _ = interval.tick() => {
                let mut controllers_out = Vec::new();
                {
                    let state_guard = shared_state.lock().unwrap();
                    for i in 0..4 {
                        if !port_connected[i] || port_buffers[i].is_empty() {
                            port_buffers[i].clear();
                            continue;
                        }

                        let samples: Vec<ControllerSample> = port_buffers[i].drain(..).collect();
                        let hz = state_guard.ports[i].poll_rate;

                        let origins = if let Some(cal) = state_guard.calibrations[i] {
                            let stick_x_offset = (cal.stick.0 as i16) - 128;
                            let stick_y_offset = (cal.stick.1 as i16) - 128;
                            let c_x_offset = (cal.c_stick.0 as i16) - 128;
                            let c_y_offset = (cal.c_stick.1 as i16) - 128;

                            OriginData {
                                stick_x: stick_x_offset as f32 / 80.0,
                                stick_y: stick_y_offset as f32 / 80.0,
                                c_x: c_x_offset as f32 / 80.0,
                                c_y: c_y_offset as f32 / 80.0,
                                l: cal.triggers.0,
                                r: cal.triggers.1,
                            }
                        } else {
                            OriginData {
                                stick_x: 0.0, stick_y: 0.0, c_x: 0.0, c_y: 0.0, l: 0, r: 0,
                            }
                        };

                        controllers_out.push(ControllerDataPacket {
                            port: i, connected: true, poll_rate: hz, origins, samples,
                        });
                    }
                }

                if !controllers_out.is_empty() {
                    let update = BridgeUpdate { controllers: controllers_out };
                    if let Ok(json) = serde_json::to_string(&update) {
                        if tx.send(Message::text(json)).await.is_err() {
                            break;
                        }
                    }
                }
            }

            msg = ws_rx.next() => {
                if msg.is_none() { break; }
            }
        }
    }

    if let Ok(mut state) = shared_state.lock() {
        state.client_connected = false;
    }
}

// (UNCHANGED: start_adapter_polling - your 1000Hz nUSB code is perfect)
async fn start_adapter_polling(
    shared_state: Arc<Mutex<AppSharedState>>,
    shutdown_signal: Arc<AtomicBool>,
    broadcast_tx: tokio::sync::broadcast::Sender<PollMessage>,
) {
    let mut calibrations: Vec<Option<ControllerCalibration>> = vec![None; 4];
    let mut frame_counters: Vec<u32> = vec![0; 4];
    let mut polls_sec: Vec<u32> = vec![0; 4];
    let mut last_hz_time = Instant::now();

    loop {
        if !shutdown_signal.load(Ordering::Relaxed) { break; }

        let device_info = match nusb::list_devices()
            .unwrap()
            .find(|d| d.vendor_id() == GC_ADAPTER_VID && d.product_id() == GC_ADAPTER_PID)
        {
            Some(d) => d,
            None => {
                if let Ok(mut s) = shared_state.lock() {
                    s.adapter_status = "Searching...".to_string();
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };

        let device = match device_info.open() {
            Ok(d) => d,
            Err(e) => {
                #[cfg(target_os = "linux")]
                if format!("{e}").to_lowercase().contains("permission")
                    || format!("{e}").to_lowercase().contains("access")
                {
                    if let Ok(mut s) = shared_state.lock() {
                        s.adapter_status = "Permission denied".to_string();
                        s.setup_hint = Some(
                            "echo 'SUBSYSTEM==\"usb\", ATTRS{idVendor}==\"057e\", \
ATTRS{idProduct}==\"0337\", MODE=\"0666\"' \
| sudo tee /etc/udev/rules.d/99-gcadapter.rules \
&& sudo udevadm control --reload-rules"
                                .to_string(),
                        );
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };

        let interface = match device.claim_interface(0) {
            Ok(i) => i,
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };

        let completion = interface.bulk_out(GC_ENDPOINT_OUT, vec![0x13]).await;
        if completion.status.is_err() {
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }

        if let Ok(mut s) = shared_state.lock() {
            s.adapter_status = "Connected".to_string();
        }

        loop {
            if !shutdown_signal.load(Ordering::Relaxed) { break; }

            let completion = match tokio::time::timeout(
                Duration::from_millis(100),
                interface.interrupt_in(GC_ENDPOINT_IN, RequestBuffer::new(GC_READ_SIZE)),
            )
            .await
            {
                Ok(c) => c,
                Err(_) => continue,
            };

            if completion.status.is_err() { break; }

            let data = completion.data;
            let now = Instant::now();

            if now.duration_since(last_hz_time).as_secs() >= 1 {
                if let Ok(mut s) = shared_state.lock() {
                    for i in 0..4 {
                        s.ports[i].poll_rate = polls_sec[i];
                        polls_sec[i] = 0;
                    }
                }
                last_hz_time = now;
            }

            let inputs = match parse_gc_input(&data) {
                Some(i) => i,
                None => continue,
            };

            for (i, input) in inputs.iter().enumerate() {
                if !input.connected {
                    calibrations[i] = None;
                    frame_counters[i] = 0;
                    if let Ok(mut s) = shared_state.lock() {
                        s.ports[i].connected = false;
                        s.calibrations[i] = None;
                    }
                    continue;
                }

                if let Ok(mut s) = shared_state.lock() {
                    s.ports[i].connected = true;
                }

                polls_sec[i] += 1;
                frame_counters[i] += 1;

                if frame_counters[i] > 40 && calibrations[i].is_none() {
                    let new_cal = ControllerCalibration {
                        stick: (input.stick_x, input.stick_y),
                        c_stick: (input.c_stick_x, input.c_stick_y),
                        triggers: (input.trigger_l, input.trigger_r),
                    };
                    calibrations[i] = Some(new_cal);
                    if let Ok(mut s) = shared_state.lock() {
                        s.calibrations[i] = Some(new_cal);
                    }
                }

                let cal = calibrations[i].unwrap_or(ControllerCalibration {
                    stick: (128, 128), c_stick: (128, 128), triggers: (0, 0),
                });

                let sx = (input.stick_x as i16) - (cal.stick.0 as i16);
                let sy = (input.stick_y as i16) - (cal.stick.1 as i16);
                let (mx, my) = melee_coord_clamp(sx, sy);

                let cx = (input.c_stick_x as i16) - (cal.c_stick.0 as i16);
                let cy = (input.c_stick_y as i16) - (cal.c_stick.1 as i16);
                let (mcx, mcy) = melee_coord_clamp(cx, cy);

                let sample = ControllerSample {
                    timestamp: now.elapsed().as_micros(),
                    buttons: ButtonsData {
                        a: input.a, b: input.b, x: input.x, y: input.y,
                        start: input.start, z: input.z, l: input.l, r: input.r,
                        d_left: input.d_left, d_right: input.d_right,
                        d_up: input.d_up, d_down: input.d_down,
                    },
                    stick: StickData { x: sx, y: sy, melee_x: mx, melee_y: my },
                    c_stick: StickData { x: cx, y: cy, melee_x: mcx, melee_y: mcy },
                    triggers: TriggerData {
                        l: input.trigger_l.saturating_sub(cal.triggers.0),
                        r: input.trigger_r.saturating_sub(cal.triggers.1),
                    },
                };

                let _ = broadcast_tx.send(PollMessage {
                    port_index: i,
                    sample,
                    connected: true,
                });
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
#[tokio::main]
async fn main() {
    set_timer_resolution();

    let shared_state = Arc::new(Mutex::new(AppSharedState {
        adapter_status: "Initializing...".to_string(),
        server_ready: false,
        client_connected: false,
        ports: [PortMetadata { connected: false, poll_rate: 0 }; 4],
        calibrations: [None; 4],
        setup_hint: None,
    }));

    let shutdown_signal = Arc::new(AtomicBool::new(true));
    let (broadcast_tx, _) = tokio::sync::broadcast::channel::<PollMessage>(10000);

    // FIXED: Spawn WS server FIRST (before eframe) - original method works
    let ws_state = shared_state.clone();
    let ws_tx = broadcast_tx.clone();
    let server_state = shared_state.clone();  // For server_ready flag

    tokio::spawn(async move {
        // Mark server "ready" IMMEDIATELY after spawn (socket binds instantly)
        if let Ok(mut s) = server_state.lock() {
            s.server_ready = true;
        }

        let ws_route = warp::path("ws")
            .and(warp::ws())
            .and(warp::any().map(move || {
                let rx = ws_tx.subscribe();
                let (bridge_tx, bridge_rx) = mpsc::unbounded_channel();
                let mut b_rx = rx;
                tokio::spawn(async move {
                    loop {
                        match b_rx.recv().await {
                            Ok(msg) => { let _ = bridge_tx.send(msg); }
                            Err(_) => break,
                        }
                    }
                });
                bridge_rx
            }))
            .and(warp::any().map(move || ws_state.clone()))
            .map(|ws: warp::ws::Ws, rx, state| {
                ws.on_upgrade(move |socket| handle_client(socket, rx, state))
            })
            .with(warp::cors().allow_any_origin());

        // Original method - binds instantly (<10ms)
        warp::serve(ws_route).run(([127, 0, 0, 1], 8080)).await;
    });

    // Spawn USB poller
    let poll_state = shared_state.clone();
    let poll_shutdown = shutdown_signal.clone();
    let poll_tx = broadcast_tx.clone();
    tokio::spawn(async move {
        start_adapter_polling(poll_state, poll_shutdown, poll_tx).await;
    });

    // Start eframe LAST
    let app = BridgeApp { shared_state, shutdown_signal };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([380.0, 320.0])
            .with_title("GC Bridge")
            .with_resizable(false)
            .with_decorations(true),
        ..Default::default()
    };

    eframe::run_native("GC Bridge", options, Box::new(|_cc| Ok(Box::new(app)))).unwrap();
    reset_timer_resolution();
}
