# VHS Demo Recordings

This folder contains [VHS](https://github.com/charmbracelet/vhs) tape files and their rendered outputs used for demo purposes on the website.

## Contents

- `*.tape` - VHS tape scripts that define terminal recordings
- `*.gif`, `*.webm`, `*.mp4` - Rendered outputs from the tape files

## Usage

To regenerate a recording:

```bash
cd docs/vhs
vhs demo.tape
```

## Requirements

- [VHS](https://github.com/charmbracelet/vhs) (`brew install vhs`)
- [ttyd](https://github.com/tsl0922/ttyd) (`brew install ttyd`)
- [ffmpeg](https://ffmpeg.org/) (`brew install ffmpeg`)
