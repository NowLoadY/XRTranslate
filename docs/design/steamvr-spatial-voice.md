# SteamVR spatial voice interaction

## Interaction

The overlay adds a grabbable microphone and two optional spatial buttons in the
companion's shared SteamVR tracking space. It is not another settings panel,
editor or history browser.
The microphone controls XRTranslate recognition, not the game's audio route.
Desktop settings configure the feature; spatial voice controls are off by default.

Grip holds the microphone. Mouth distance hysteresis controls recognition;
release returns the microphone to its dock, and tracking loss stops recognition.
Send and Edit never toggle the microphone, close a capture batch or restart the
pipeline. Holding the microphone near the mouth keeps recognition running.

The optional **Review before sending** setting is off by default. When off,
OSC keeps its existing behavior and the two review buttons are hidden. When on,
microphone phrases enter one review slot and require an explicit Send action.
The held microphone can strike a button, allowing operation with one controller.
A front strike requires at least 0.40 m/s inward speed and a mainly inward
direction; slow contact, side swipes and stale tracking frames do not activate it.
Each contact is consumed once, then rearms after separation. Slow approach plus
Grip grabs a button for placement; release keeps its position and orientation.
Grabbing never activates a button. Icons, motion and haptics supplement color.

## Shared space and appearance

The objects are peers of the Avatar, not children of its model or body transform.
Their anchor starts 45 cm horizontally ahead and 35 cm below the headset, with
Send, Edit and the microphone arranged in a compact row within reach. Avatar
turns, visits, animation and scale do not move them. Head turns do not move them
either. Walking more than 35 cm from the placement origin for 0.4 seconds recenters
the group; contact or button manipulation holds the anchor still. Placed button
offsets remain intact. The microphone follows the hand while held and returns to
its dock on release.

Props use the Avatar's existing stereo mesh renderer and clay material: rounded
forms, cream/mist-blue/rose accessories, soft colored contours and restrained
cel shading. Buttons are oval cushions with raised symbols; the microphone has
a rounded head, three shallow grille lines and a soft grip. There are no extra
faces, screens or decorative controls.

Unavailable buttons are neutral gray and cannot fire; they remain grabbable.
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
- SteamVR owns tracking, grabs, button contact, presentation and feedback. It sends
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
