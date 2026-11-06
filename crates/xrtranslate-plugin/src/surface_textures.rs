//! Lossless texture deltas; avoid a JSON number allocation for every color channel.
use base64::{Engine, engine::general_purpose::STANDARD};
use egui::{
    ColorImage, TextureId, TextureOptions, Vec2,
    epaint::{ImageData, ImageDelta},
};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::io::{Read, Write};

type Textures = Vec<(TextureId, Vec<ImageDelta>)>;
type Encoded = Vec<(TextureId, Vec<Delta>)>;

#[derive(Serialize, Deserialize)]
struct Delta {
    size: [usize; 2],
    source_size: Vec2,
    #[serde(with = "pixels")]
    pixels: Vec<u8>,
    options: TextureOptions,
    pos: Option<[usize; 2]>,
}

pub fn serialize<S: Serializer>(textures: &Textures, serializer: S) -> Result<S::Ok, S::Error> {
    let encoded: Result<Encoded, std::io::Error> = textures
        .iter()
        .map(|(id, deltas)| {
            let deltas = deltas
                .iter()
                .map(|delta| -> std::io::Result<Delta> {
                    let ImageData::Color(image) = &delta.image;
                    let bytes: Vec<u8> = image
                        .pixels
                        .iter()
                        .flat_map(|pixel| pixel.to_array())
                        .collect();
                    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
                    encoder.write_all(&bytes)?;
                    Ok(Delta {
                        size: image.size,
                        source_size: image.source_size,
                        pixels: encoder.finish()?,
                        options: delta.options,
                        pos: delta.pos,
                    })
                })
                .collect::<Result<_, _>>()?;
            Ok((*id, deltas))
        })
        .collect();
    encoded
        .map_err(serde::ser::Error::custom)?
        .serialize(serializer)
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Textures, D::Error> {
    let encoded = Encoded::deserialize(deserializer)?;
    let mut total = 0usize;
    encoded
        .into_iter()
        .map(|(id, deltas)| {
            let deltas = deltas
                .into_iter()
                .map(|delta| {
                    let length = delta.size[0]
                        .checked_mul(delta.size[1])
                        .and_then(|pixels| pixels.checked_mul(4))
                        .filter(|length| *length <= 64 * 1024 * 1024)
                        .ok_or_else(|| serde::de::Error::custom("Invalid texture dimensions"))?;
                    total += length;
                    if total > 128 * 1024 * 1024 {
                        return Err(serde::de::Error::custom("Texture updates are too large"));
                    }
                    let mut pixels = Vec::new();
                    ZlibDecoder::new(delta.pixels.as_slice())
                        .take(length as u64 + 1)
                        .read_to_end(&mut pixels)
                        .map_err(serde::de::Error::custom)?;
                    if pixels.len() != length {
                        return Err(serde::de::Error::custom("Invalid texture pixels"));
                    }
                    let mut image = ColorImage::from_rgba_premultiplied(delta.size, &pixels);
                    image.source_size = delta.source_size;
                    Ok(ImageDelta {
                        image: image.into(),
                        options: delta.options,
                        pos: delta.pos,
                    })
                })
                .collect::<Result<_, D::Error>>()?;
            Ok((id, deltas))
        })
        .collect()
}

mod pixels {
    use super::*;

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            STANDARD.encode(bytes).serialize(serializer)
        } else {
            serializer.serialize_bytes(bytes)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        if deserializer.is_human_readable() {
            STANDARD
                .decode(String::deserialize(deserializer)?)
                .map_err(serde::de::Error::custom)
        } else {
            serde_bytes::ByteBuf::deserialize(deserializer).map(serde_bytes::ByteBuf::into_vec)
        }
    }
}
