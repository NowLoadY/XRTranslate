# SteamVR spatial voice interaction

## Interaction

The left controller carries two optional buttons along the wrist-to-forearm
axis. Its grip center controls voice input directly by distance to the headset;
there is no 3D microphone prop or grab action. Spatial controls are off by default.
The optional OSC microphone sync also controls the game's microphone through its
existing input API.

Entering within 30 cm of the head opens input after 80 ms. Moving beyond 38 cm
closes it after 160 ms; the separate enter/exit distances avoid boundary chatter.
Losing tracking or opening the SteamVR dashboard closes input. Manual Stop or
game-side mute requires the left controller to leave the range before reentry can
open it again. Review, Send and Edit never change this proximity gate.

A small green dot at the subtitle HUD's top-left corner slowly pulses over a
2.4-second cycle while proximity input is active. It remains available when no
subtitles or review buttons are visible. Its texture is uploaded once; native
alpha controls the pulse.

Default bindings use the controller's official grip pose (Vive/Index `grip`,
Touch `openxr_grip`). The right controller presses the left-arm buttons. Normal
front contact triggers once; no speed threshold or grip press is required.
Withdraw before pressing again. Tracking that resumes inside a button cannot
activate it until withdrawal. Button-local samples keep pressing stable while
the left arm moves. Buttons have fixed positions and cannot be grabbed.

The optional **Review before sending** setting is off by default. When off, OSC
keeps its existing behavior and the two buttons are hidden. When on, microphone
phrases enter one review slot and require an explicit Send action. Pressing Edit
enters voice editing; pressing it again finishes editing and retains the current
result, including when an edit request is still running. Late results are ignored.
Leaving voice range also finishes editing and retains applied changes. Reentry
starts ordinary speech input; press Edit explicitly to enter editing again.
Icons, press motion and haptics supplement color.

## Game microphone sync

OSC's optional **Sync game microphone** replaces the former **Pause while muted**
switch and keeps its saved preference. With VRChat's **Toggle Voice** enabled,
explicit microphone on/off actions (including spatial near/far and Stop Translation)
also request the matching game state. Starting or failing an inference task and
initially entering spatial interaction do not change the game's microphone.

The existing OSC transport reads `MuteSelf` and sends `/input/Voice` press/release
only when a requested state differs from a known game state. It discovers the
initial state through OSCQuery, serializes toggles, and treats its own feedback as
confirmation. Unknown state waits for feedback; an unconfirmed toggle is not
blindly retried. Game-side muting stops local microphone recognition and resets
the proximity gate. Moving the left controller out of range and back in is a new
open request. Game-side unmuting does not start local recognition.

The host connects these events. OSC owns game state and input delivery; SteamVR
owns proximity and button contact. Neither plugin calls the other. Only microphone audio
uses the mute gate; system audio remains independent. Send, Edit and review state
do not control either microphone.

## Shared space and appearance

The buttons are independent of the Avatar model and follow the left controller's
grip pose. The anchor is offset 10 cm along grip +Y and 8 cm toward the thumb
(-Z), with the button fronts facing grip +Y. Send and Edit sit 12 cm apart along
grip +Z toward the forearm. Avatar animation, scale and visits do not move them.
Losing left-controller tracking hides the buttons and the HUD dot and stops input.

Buttons retain the Avatar's stereo mesh renderer, oval cushions, raised symbols,
clay material and cream/mist-blue/rose palette. Their existing geometry is kept.
Unavailable buttons are neutral gray and cannot fire.
Edit reads the selected model's `supports_chat` capability from the existing model
metadata. The Ask endpoint uses the same value, independent of translation prompt
format. Switching to an incapable model ends editing and keeps the current text;
sending it through OSC remains independent. No capability polling service is added.

## One latest-phrase slot

The host supplies stable microphone utterances and deduplicates repeated result
updates. Each new phrase replaces the review slot; phrases are never appended
or accumulated into a message queue. The slot shows recognized or translated
text according to the active pipeline mode.

Edit freezes the current slot. Each subsequent recognized phrase is a separate
Ask instruction, applied to that frozen message. Instructions are processed
in order; each prompt uses the result of the previous completed edit. Recognition
continues throughout. Editing consumes the ASR source text, and the review gate
prevents automatic microphone Chatbox output. Existing inference, captions and
other output settings are not reconfigured by this feature.

Editing stays active after each result. Tap Edit again to finish editing and
retain applied changes. Pending instructions and late Ask results are discarded.
An Ask failure retains the current text. The next ordinary phrase can replace the
slot after editing ends. No undo history or second confirmation button is needed.

`voice_interaction/prompt.rs` follows vocabulary examples' feature-owned Ask
request and result checks, and Prompt Studio's separation of task, current input
and references. Each request contains a short fixed edit task plus JSON-quoted
original speech, original translation language, latest accepted message and this
spoken change. The current message carries its existing style and language;
the change may alter either. The original language is a reference, so later edits
keep a newly requested language. Prompts are built when each instruction starts,
after the preceding result is accepted. Empty results and obvious request echoes
retain the text; unchanged results retain its version. Ask transport and the
translation graph stay unchanged, with no extra model call or style/history store.

Send submits the currently displayed slot through OSC, exits editing and ignores
pending edit results. It does not alter capture or the translation pipeline.
Later ordinary recognition immediately resumes replacing the slot, including while
the submission is in flight. A receipt for an older slot never clears a newer one.
Successful submission marks that version sent; failure leaves it available to retry.
OSC owns formatting, length limits and transport; oversized text remains editable.
A local submission receipt does not claim another player displayed the message.

## Ownership

- `voice_interaction.rs` owns only the replaceable slot, edit state, action
  availability and Ask/Publish effects. It has no audio sessions or plugin calls.
- `voice_interaction_host.rs` observes microphone results, routes editing ASR,
  invokes the existing Ask and OSC services, and returns their results.
- SteamVR owns tracking, proximity, button contact, presentation and feedback. It sends
  intents and receives a snapshot; it does not invoke OSC or Ask directly.
- `interaction.rs` owns poses and gestures; `voice_props.rs` only builds meshes.
- The existing capture and pipeline owners keep their lifecycles. The microphone's
  near/far intent is handled separately from review actions.

Local increasing operation numbers reject cancelled replies. They are not hashes,
authentication or a new messaging framework. Disabling review exits editing and
restores normal OSC behavior. An already submitted publication cannot be recalled.
The controller keeps only one in-flight Ask plus pending spoken edit instructions;
there is no finite-capture controller, preroll system or recording queue.

## Extending direct actions

Add behavior to the controller, a button specification to SteamVR, and a host
effect handler only when a new capability is needed. Keep existing positions
stable; more buttons do not imply pages, menus or a generic plugin registry.
These local stereo meshes are composited by SteamVR, not shared VRChat objects;
they do not collide with game geometry. The existing OpenVR loader is Windows-only.
