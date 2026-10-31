# vr-imageview

A tool that lets you display images directly\* to the panels of a VR headset.

Sometimes you need a reproducible way to diagnose display issues with distortion, display stream compression, whatever. This makes it easier.

\* Still goes through the compositor of your OpenXR runtime of choice.

## Usage

`cargo run -- path/to/image`

#### Controls
- Pan: Left Stick
- Zoom: Right Stick
- Rotate: Left & Right Triggers
- Change clamp mode: X
- Toggle simulated perspective: Y
- Reset view: A
- Toggle square aspect ratio: B