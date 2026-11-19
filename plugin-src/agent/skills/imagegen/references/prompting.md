# Prompting

Shape the brief in the order that helps the artist see the image. Useful fields include intended use, subject and action, setting, medium, framing, light, materials, exact text, and constraints. Pick the fields that affect the result.

## Specificity and framing

Keep the user's chosen objects, colors, style and text. In an open brief, infer practical details from the deliverable. A cover might need a calm title area. A timeline overlay might need a clear silhouette, generous edge padding and real transparency. For photography, specify framing, viewpoint, light direction and a few convincing real-world textures. Add props or characters only when the brief calls for them.

Use these established generation use cases to orient the prompt:

- **photorealistic-natural**, **historical-scene**. Give place, time, viewpoint, human action or material details when relevant.
- **product-mockup**, **ads-marketing**, **logo-brand**. Specify shape, silhouette, materials, exact authorized copy and intended size.
- **illustration-story**, **stylized-concept**. Describe what happens in the frame, the medium, and any visual anchor the next shot must retain.
- **ui-mockup**, **infographic-diagram**, **productivity-visual**, **scientific-educational**. Give the reading order, labels, relationships and intended scale. Verify every visible label and relationship.

## Reference images and edits

Label inputs by their order and role. For a composite, specify which subject moves into which base scene and how scale, perspective and shadows should match. For reference-guided generation, identify what to borrow and what the new scene should contain.

Choose the edit instruction to fit the change:

- **precise-object-edit**, **text-localization**. Identify the object or original words and their exact replacement. Preserve the geometry, spacing, light and surrounding texture.
- **identity-preserve**, **style-transfer**, **sketch-to-render**. Identify the identity features, structure or artistic cues to carry forward. Describe the changes separately.
- **lighting-weather**, **compositing**. Give lighting direction, time, atmosphere, scale and framing. Anchor the existing scene to reduce drift.
- **background-extraction**. Preserve hair, fine edges, internal holes and subject detail. Request alpha in the tool, inspect edge halos and check the returned transparency measurement.

## Text, composition and iteration

Quote exact words, capitalization and punctuation. Specify position and lettering only when needed. Inspect at the intended viewing size. Captions that must remain editable belong on the timeline. For an in-image inscription, edit the rendered image if the spelling fails.

Review the composition both as a full frame and in its intended use. A sequential edit should change one thing at a time and repeat the details that must carry forward. Keep returned paths so later calls can edit or reuse the selected image.
