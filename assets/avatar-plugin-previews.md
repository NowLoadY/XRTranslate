# Avatar plugin previews

The Avatar and welcome pages embed these 1536 × 1024 concept illustrations. They were
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
