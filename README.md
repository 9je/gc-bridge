# GC Bridge

A lightweight desktop app that reads your **Nintendo GameCube adapter** over USB and streams live controller data to any browser via a local WebSocket server.

Built with Rust. Works on Windows, Linux, and macOS.

---

## What it does

GC Bridge runs in your system tray, detects a connected GameCube adapter, polls it at up to **1000 Hz**, and exposes the data on `ws://127.0.0.1:8080/ws`. Any webpage or browser app connecting to that address receives a real-time JSON stream of button presses, analog stick positions, and trigger values for all four ports.

Use it to build:
- **Controller overlays** for streams
- **Training tools** and input displays for Melee / PM / other titles
- **Visualizers, reaction time trackers, or any web app** that needs raw GC input

---

## Download

Grab the latest binary for your platform from the [Releases](../../releases) page:

| Platform | File |
|---|---|
| Windows | `gc-bridge-windows-x64.exe` |
| Linux (x64) | `gc-bridge-linux-x64` |
| macOS Intel | `gc-bridge-macos-intel` |
| macOS Apple Silicon | `gc-bridge-macos-apple-silicon` |

---

## Setup

### Windows

Run `gc-bridge-windows-x64.exe`. No additional setup required.

> If Windows Defender flags the binary, click **More info → Run anyway**. The app has no installer and makes no system changes.

### Linux

1. Make the binary executable:
   ```bash
   chmod +x gc-bridge-linux-x64
   ```

2. Add a udev rule so the app can access the adapter without root:
   ```bash
   echo 'SUBSYSTEM=="usb", ATTRS{idVendor}=="057e", ATTRS{idProduct}=="0337", MODE="0666"' \
     | sudo tee /etc/udev/rules.d/99-gcadapter.rules
   sudo udevadm control --reload-rules
   ```

3. Unplug and replug the adapter, then run the app:
   ```bash
   ./gc-bridge-linux-x64
   ```

> If you skip the udev rule, GC Bridge will detect the denied permission and show a **Copy setup command** button in the UI.

### macOS

1. Make the binary executable:
   ```bash
   chmod +x gc-bridge-macos-*
   ```

2. **Right-click** the binary and choose **Open** the first time to bypass macOS Gatekeeper. After that, you can double-click normally or run from the terminal.

---

## Connecting from the Browser

Once running, open a WebSocket connection to:

```
ws://127.0.0.1:8080/ws
```

```js
const ws = new WebSocket("ws://127.0.0.1:8080/ws");

ws.onmessage = (event) => {
  const data = JSON.parse(event.data);
  for (const controller of data.controllers) {
    if (controller.connected) {
      console.log(`Port ${controller.port + 1}:`, controller.samples.at(-1));
    }
  }
};
```

Messages are sent at ~60 Hz. Each message contains all samples captured since the last message (typically ~16 per controller at 1000 Hz polling).

---

## WebSocket Message Format

```ts
type BridgeUpdate = {
  controllers: ControllerDataPacket[];
};

type ControllerDataPacket = {
  port: number;        // 0–3
  connected: boolean;
  poll_rate: number;   // measured Hz (typically ~1000)
  origins: OriginData; // calibrated neutral offsets
  samples: ControllerSample[];
};

type ControllerSample = {
  timestamp: number;   // microseconds since adapter connection
  buttons: {
    a: boolean; b: boolean; x: boolean; y: boolean;
    start: boolean; z: boolean;
    l: boolean; r: boolean;           // digital L/R press
    d_left: boolean; d_right: boolean;
    d_up: boolean; d_down: boolean;
  };
  stick: StickData;
  c_stick: StickData;
  triggers: {
    l: number;  // 0–255, offset from calibrated neutral
    r: number;
  };
};

type StickData = {
  x: number;        // raw offset from calibrated center, roughly –128 to 127
  y: number;
  melee_x: number;  // Melee-clamped coordinate, –1.0 to 1.0
  melee_y: number;
};

type OriginData = {
  stick_x: number; stick_y: number;  // center offset as Melee float
  c_x: number;     c_y: number;
  l: number;       r: number;        // trigger neutral raw value
};
```

### Calibration

On startup, GC Bridge waits ~40 frames per port before locking in a calibration. Keep the sticks and triggers at neutral when first plugging in a controller. The `origins` field in each packet reports the detected offsets so your app can apply them independently if needed.

### Example message

```json
{
  "controllers": [{
    "port": 0,
    "connected": true,
    "poll_rate": 1000,
    "origins": { "stick_x": 0.0125, "stick_y": -0.025, "c_x": 0.0, "c_y": 0.0, "l": 0, "r": 0 },
    "samples": [{
      "timestamp": 4823910,
      "buttons": { "a": true, "b": false, "x": false, "y": false, "start": false,
                   "z": false, "l": false, "r": false,
                   "d_left": false, "d_right": false, "d_up": false, "d_down": false },
      "stick":   { "x": 1, "y": -2, "melee_x": 0.0125, "melee_y": -0.025 },
      "c_stick": { "x": 0, "y":  0, "melee_x": 0.0,    "melee_y":  0.0   },
      "triggers": { "l": 0, "r": 0 }
    }]
  }]
}
```

---

## Building from Source

**Prerequisites:** Rust (install via [rustup.rs](https://rustup.rs))

### Linux — extra system packages needed

```bash
sudo apt-get install -y \
  libgl1-mesa-dev libx11-dev libxrandr-dev \
  libxi-dev libxcursor-dev libxinerama-dev \
  libudev-dev pkg-config
```

### Build

```bash
git clone https://github.com/9je/gc-bridge
cd gc-bridge
cargo build --release
# binary: target/release/gc-bridge  (or gc-bridge.exe on Windows)
```

### Release builds (all platforms via CI)

Push a git tag to trigger the GitHub Actions workflow that produces binaries for all platforms:

```bash
git tag v1.0.0
git push origin --tags
```

The workflow builds natively on Windows, Linux, and both Intel/Apple Silicon macOS runners and attaches the binaries to the GitHub release.

---

## How it works

```
GC Adapter (USB)
      │
      │  USB interrupt transfers @ ~1000 Hz
      ▼
  GC Bridge (Rust)
      │  nusb — pure-Rust async USB
      │  Calibrates sticks/triggers on connect
      │  Computes Melee coordinates
      │
      │  Broadcasts to all connected WebSocket clients @ ~60 Hz
      ▼
  ws://127.0.0.1:8080/ws
      │
      ▼
  Your browser app
```

The adapter is a standard Nintendo GameCube Adapter (USB, VID `057e` PID `0337`). It works with official Nintendo adapters and most third-party equivalents (MayFlash in PC mode, etc.).

---

## Supported Adapters

| Adapter | Support |
|---|---|
| Official Nintendo GC Adapter (Wii U / Switch) | ✅ |
| MayFlash 4-port (PC Mode) | ✅ |
| MayFlash 2-port | ✅ |
| Other USB GC adapters using VID `057e` / PID `0337` | ✅ |

---

## License

MIT
