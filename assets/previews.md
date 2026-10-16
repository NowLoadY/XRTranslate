# Preview assets

## Application screenshots

The welcome showcase uses 1080 × 720 captures of the application, bundled as lossless WebP images.

| Image | Content |
|---|---|
| `showcase-translation.webp` | Translation input with an unsent example sentence and audio settings. |
| `showcase-media.webp` | Four original sample subtitles imported from a local SRT file. |
| `showcase-styles.webp` | Built-in translation style selection. |

## VRChat concept

`showcase-vrchat.webp` is a 1536 × 1024 imagegen illustration, bundled at WebP
quality 90. Kipfel's complete outfit and appearance follow the creator's
[full-body reference](https://booth.pximg.net/c80ffe79-d9d7-4481-bc64-40d80bcd71e6/i/5813187/a1dfcd32-162b-4172-81d4-c3d21ed0fd73_base_resized.jpg)
and [character page](https://mukumi.booth.pm/items/5813187) by もち山金魚.
Chatbox placement follows the [official example](https://wiki.vrchat.com/wiki/Chatbox),
with multiline typography checked against the [VRCT author's example](https://booth.pm/ja/items/4814313).
The two lines, `🎤 What a lovely day!` and `天气真好！`, follow the OSC plugin's
default source-first bilingual format and microphone prefix. The sunny hill is
illustrative; the image carries an AI-generated concept label.

## Big Walk concept

`showcase-bigwalk.webp` is a 1536 × 1024 imagegen illustration, bundled at WebP
quality 90 for the welcome showcase. Characters and coastal scenery follow the
[official screenshots](https://bigwalk.game/presskit/) and the hiking scene at
00:24–00:26 in the [release-date trailer](https://youtu.be/c_uvpWSlBV0).
The floating, outlined chat lettering follows the
[BigTalk ConversationStarters example](https://cdn.vixen.link/62v9gr/example.png).
The waveform bubble and arrow illustrate voice-to-text translation. The image
carries an AI-generated concept label.

## Avatar concepts

The Avatar page and welcome showcase share these 1536 × 1024 illustrations. They were
generated with imagegen using the references below; they are not runtime
screenshots. Character appearance follows complete visual references, while
poses, lighting and scenery are illustrative. Each image carries a concept label.

| Image | Characters and visual references | Display |
|---|---|---|
| `avatar-plugin-preview.jpg` | [Kipfel](https://mukumi.booth.pm/items/5813187), [Mamehinata](https://mukumi.booth.pm/items/4340548), [Usasaki](https://mukumi.booth.pm/items/3550881) by もち山金魚; copied from the [plugin's preview](https://github.com/NowLoadY/XRT-Avatar-Plugin/blob/main/docs/adapters/vrchat/images/board-placement-concept.jpg) | VRChat: text on user-assembled boards. |
| `avatar-plugin-chilloutvr.webp` | [YAAvali by DragonSkyRunner](https://dragonskyrunner.gumroad.com/l/YAAvali), gray avatar with orange wraps in the [author's full-body group image](https://public-files.gumroad.com/8m6dvf9ucwh3kkvaqcj64xvc317s) | ChilloutVR: smooth glyph text on an avatar-attached board. |
| `avatar-plugin-overte.webp` | [Woody](https://docs.overte.org/en/latest/explore/personalize/change-avatar.html) by High Fidelity: [full-body reference](https://download.tuxfamily.org/overte/content/Bazaar/Avatars/woody/img/Woody.JPG), [asset metadata](https://download.tuxfamily.org/overte/content/Bazaar/Avatars/woody/resource.json); lakeside terrace from the [Overte gallery](https://overte.org/gallery.html), [scene reference](https://overte.org/_images/overte-snap-by-X74hc595-on-2023-03-11_19-25-27.jpg) | Overte: avatar-hosted native Text entity with a light background. |
| `avatar-plugin-resonite.webp` | [The Coconut](https://wiki.resonite.com/Default_Avatar) by Ryuvi and the Resonite Art team; [Cloud Home reference](https://wiki.resonite.com/File:Default_Resonite_Avatar.jpg) uploaded by Mint Shock | Resonite: native TextRenderer, with user-selected font and material. The character consists of a floating head and two hands. |
| `avatar-plugin-secondlife.webp` | [Avatar Welcome Pack](https://community.secondlife.com/news/featured-news/introducing-the-avatar-welcome-pack-r11185/), official reference photography credited to LeLUTKA; [complete outfits](https://media.invisioncic.com/Mseclife/monthly_2025_04/AvatarWelcomePack.jpg.c05984ce1a50640d68a85f384571c183.jpg); native text appearance from the [community linkset example](https://community.secondlife.com/forums/topic/420873-linking-items-without-breaking-the-link-order/) | Second Life: small, viewer-facing hover text above an attachment prim. |

## Rendering references

- **ChilloutVR:** [Avatar parameters](https://docs.chilloutvr.net/cck/lua/api/avatar-api/)
  and the [Avatar component list](https://docs.chilloutvr.net/cck/avatar/avatar-component-whitelist/)
  support the plugin's Lua receiver and shared glyph material.
- **Overte:** [Entities API](https://apidocs.overte.org/Entities.html) supports
  avatar-hosted Text entities. The pictured horizontal panel follows the plugin's
  light background and dark, left-aligned text; its placement is editable.
- **Resonite:** [TextRenderer](https://wiki.resonite.com/Component:TextRenderer)
  supplies native scene text. The plugin binds its dynamic text source through
  [ResoniteLink](https://github.com/Yellow-Dog-Man/ResoniteLink); a solid backing
  object is optional and is not created by the text preset.
- **Second Life:** [`llSetText`](https://wiki.secondlife.com/wiki/LlSetText)
  supplies the current attachment display. The pictured small white lettering
  floats above its prim; surface lettering would require a separate renderer.

The four WebP assets retain the generated composition and dimensions, encoded at
quality 90 for bundling. They use the same sample message as the VRChat image.
