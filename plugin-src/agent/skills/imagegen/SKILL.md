---
name: imagegen
description: Generate or edit raster images with the configured OpenAI Agent. Use for new artwork, photographic scenes, textures, transparent cutouts, and reference-based image edits.
---

# Image generation and editing

Use `image.generate` for each requested bitmap. The application sends prompts and selected image pixels through the current OpenAI API-key Agent connection. It stages each returned PNG in a unique folder under the operating system's temporary `XRTranslate/generated_images` directory and provides the absolute path and measured dimensions. Import the selected artwork for a durable project copy. Use native titles, layout and editable effect nodes when the requested asset belongs in the timeline's editable typography or motion system.

## Choose the input and action

- **New image.** Supply a prompt and omit `images`. To generate a new subject using reference material for style, mood or composition, pass the ordered absolute paths and set `action` to `generate`.
- **Edit.** Put the existing local image or a previous output's returned path first in `images`. Add supporting images in order and label each one's role in the prompt. Set `action` to `edit`. Say which change to make and which features to preserve. Review provided project media with `media.render(asset_ids)` when its appearance matters. The tool accepts PNG, JPEG and WebP.
- **Several assets.** Use one prompt and one call for each asset or variant. Keep the approved image's path when the next shot needs the same character, scene or materials. Each result is a separate file.
- **Transparent output.** Set `transparent_background` to true and describe the subject's edges. On edits the tool preserves an input target's transparency by default. Check `has_transparency`, the warning if present, and the preview. An opaque result needs correction before use as a cutout. The checkerboard in a review thumbnail helps inspect alpha edges. The stored PNG retains its original pixels.

## Prompt and review

Keep detailed user briefs intact. For a short brief, add only framing, lighting, texture or negative space that the intended asset needs. Quote exact in-image copy. For edits, restate invariants at every iteration. Read `skills.read(name="imagegen", resource="references/prompting.md")` for composition, subject fidelity and production constraints. Read `references/sample-prompts.md` through the same tool for examples by asset and edit type.

Use `filename` for a descriptive basename such as `river-at-dawn.png`. Choose the intended aspect ratio with `size`, or omit it to allow the service to choose. `quality=low` suits drafts. Use `medium`, `high` or `auto` for final art, small text or delicate edits. Review the returned image for subject, crop, legibility and invariants. Make a targeted follow-up edit when needed.

## Use the image

For a preview, give the saved path and describe the observed result. For an editing project, read the current revision with `media.assets(view="project")` and import the selected output with `media.assets(revision=revision, paths=[path])`. Use the returned asset ID in the sequence and inspect the imported artwork with `media.render(asset_ids=[id])`. The import copies it into the project's material bin. Report the project asset or saved path plus the prompt or changes made. If the provider refuses a request or returns no image, surface its reason and use existing materials where that meets the brief.
