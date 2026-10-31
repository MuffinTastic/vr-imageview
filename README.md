# vr-imageview

A tool that lets you display images directly\* to the panels of a VR headset.

Sometimes you need a reproducible way to diagnose display issues with distortion, display stream compression, whatever. This makes it easier.

\* Still goes through the compositor of your OpenXR runtime of choice.

## Usage

`vr-imageview path/to/image`

(or use `cargo run -- path/to/image` after cloning the repo)

The default setting is to apply simulated perspective and use the original aspect ratio. To display any image as square and map it 1:1 with the display output, run:

`vr-imageview --square --no-perspective-correction path/to/image`

(or `vr-imageview -sn path/to/image`)

#### Controls
- Pan: Left Stick
- Zoom: Right Stick
- Rotate: Left & Right Triggers
- Change clamp mode: X
- Toggle simulated perspective: Y
- Reset view: A
- Toggle square aspect ratio: B