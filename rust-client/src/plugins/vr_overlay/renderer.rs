//! High-performance Direct2D/DirectWrite subtitle card rasterizer for VR overlay.
//!
//! Renders bilingual subtitle entries with modern dark translucent cards,
//! crisp typography, and speaker badges into a 32-bit BGRA/RGBA pixel buffer.

#[derive(Clone, Debug, PartialEq)]
pub struct VrSubtitleCard {
    pub source: String,
    pub translated: String,
    pub speaker: String,
    pub live: bool,
}

pub struct VrOverlayRenderer {
    pub width: u32,
    pub height: u32,
    #[cfg(windows)]
    d2d_factory: Option<windows::Win32::Graphics::Direct2D::ID2D1Factory>,
    #[cfg(windows)]
    dwrite_factory: Option<windows::Win32::Graphics::DirectWrite::IDWriteFactory>,
}

impl VrOverlayRenderer {
    pub fn new(width: u32, height: u32) -> Self {
        #[cfg(windows)]
        {
            use windows::Win32::Graphics::Direct2D::{
                D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1CreateFactory, ID2D1Factory,
            };
            use windows::Win32::Graphics::DirectWrite::{
                DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory, IDWriteFactory,
            };

            let d2d_factory: Option<ID2D1Factory> = unsafe {
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()
            };
            let dwrite_factory: Option<IDWriteFactory> = unsafe {
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()
            };

            Self {
                width,
                height,
                d2d_factory,
                dwrite_factory,
            }
        }
        #[cfg(not(windows))]
        {
            Self { width, height }
        }
    }

    /// Renders subtitle cards into a 32-bit pixel buffer of size `width * height * 4`.
    pub fn render(
        &self,
        cards: &[VrSubtitleCard],
        bilingual: bool,
        font_size: f32,
        opacity: f32,
    ) -> Vec<u8> {
        let buffer_size = (self.width * self.height * 4) as usize;
        let mut buffer = vec![0u8; buffer_size];

        if cards.is_empty() {
            return buffer;
        }

        #[cfg(windows)]
        {
            if let (Some(d2d), Some(dwrite)) = (&self.d2d_factory, &self.dwrite_factory) {
                match self.render_d2d(
                    d2d,
                    dwrite,
                    cards,
                    bilingual,
                    font_size,
                    opacity,
                    &mut buffer,
                ) {
                    Ok(()) => return buffer,
                    Err(e) => {
                        log::warn!("[VR Overlay] Direct2D render warning: {e:?}");
                    }
                }
            }
        }

        // Software fallback if Direct2D initialization fails
        self.render_fallback(cards, &mut buffer);
        buffer
    }

    #[cfg(windows)]
    fn render_d2d(
        &self,
        d2d: &windows::Win32::Graphics::Direct2D::ID2D1Factory,
        dwrite: &windows::Win32::Graphics::DirectWrite::IDWriteFactory,
        cards: &[VrSubtitleCard],
        bilingual: bool,
        font_size: f32,
        opacity: f32,
        out_buffer: &mut [u8],
    ) -> windows::core::Result<()> {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::Graphics::Direct2D::Common::{
            D2D_POINT_2F, D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
        };
        use windows::Win32::Graphics::Direct2D::{
            D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_NONE,
            D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES,
            D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
            D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE, ID2D1DCRenderTarget,
        };
        use windows::Win32::Graphics::DirectWrite::{
            DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS,
        };
        use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
        use windows::Win32::Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
            DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HDC, HGDIOBJ, ReleaseDC, SelectObject,
        };
        use windows::core::w;

        // RAII guard for GDI objects to prevent handle exhaustion
        struct GdiGuard {
            hdc_screen: HDC,
            hdc_mem: HDC,
            hbitmap: windows::Win32::Graphics::Gdi::HBITMAP,
            old_bitmap: HGDIOBJ,
        }

        impl Drop for GdiGuard {
            fn drop(&mut self) {
                unsafe {
                    if !self.old_bitmap.is_invalid() && !self.hdc_mem.is_invalid() {
                        SelectObject(self.hdc_mem, self.old_bitmap);
                    }
                    if !self.hbitmap.is_invalid() {
                        let _ = DeleteObject(self.hbitmap.into());
                    }
                    if !self.hdc_mem.is_invalid() {
                        let _ = DeleteDC(self.hdc_mem);
                    }
                    if !self.hdc_screen.is_invalid() {
                        ReleaseDC(None, self.hdc_screen);
                    }
                }
            }
        }

        unsafe {
            let hdc_screen = GetDC(None);
            let hdc_mem = CreateCompatibleDC(Some(hdc_screen));

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: self.width as i32,
                    biHeight: -(self.height as i32), // Top-down DIB
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };

            let mut dib_bits = std::ptr::null_mut();
            let hbitmap = CreateDIBSection(
                Some(hdc_mem),
                &bmi,
                DIB_RGB_COLORS,
                &mut dib_bits,
                None,
                0,
            )?;
            let old_bitmap = SelectObject(hdc_mem, hbitmap.into());

            // Guarantee GDI cleanup on ANY return path
            let _gdi_guard = GdiGuard {
                hdc_screen,
                hdc_mem,
                hbitmap,
                old_bitmap,
            };

            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: windows::Win32::Graphics::Direct2D::Common::D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };

            let target: ID2D1DCRenderTarget = d2d.CreateDCRenderTarget(&props)?;
            let target_rect = RECT {
                left: 0,
                top: 0,
                right: self.width as i32,
                bottom: self.height as i32,
            };
            target.BindDC(hdc_mem, &target_rect)?;

            target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);

            target.BeginDraw();
            target.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));

            let font_size = font_size.clamp(12.0, 36.0);

            // Text Formats
            let primary_format = dwrite.CreateTextFormat(
                w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                font_size,
                w!("zh-CN"),
            )?;
            primary_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;

            let secondary_format = dwrite.CreateTextFormat(
                w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                (font_size * 0.84).max(11.0),
                w!("zh-CN"),
            )?;
            secondary_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;

            let speaker_format = dwrite.CreateTextFormat(
                w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                (font_size * 0.68).max(10.0),
                w!("zh-CN"),
            )?;

            // Color Palette
            let bg_alpha = (0.78 * opacity).clamp(0.1, 0.95);
            let card_bg = target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.05,
                    g: 0.07,
                    b: 0.11,
                    a: bg_alpha,
                },
                None,
            )?;
            let history_card_bg = target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.04,
                    g: 0.06,
                    b: 0.09,
                    a: bg_alpha * 0.90,
                },
                None,
            )?;
            let border_brush = target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.28,
                    g: 0.38,
                    b: 0.52,
                    a: (0.50 * opacity).clamp(0.1, 1.0),
                },
                None,
            )?;
            let live_border_brush = target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.22,
                    g: 0.74,
                    b: 0.97,
                    a: (0.90 * opacity).clamp(0.2, 1.0),
                },
                None,
            )?;
            let live_dot_brush = target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.22,
                    g: 0.85,
                    b: 0.98,
                    a: opacity.clamp(0.2, 1.0),
                },
                None,
            )?;
            let primary_live_brush = target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: opacity.clamp(0.2, 1.0),
                },
                None,
            )?;
            let primary_hist_brush = target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.92,
                    g: 0.95,
                    b: 0.98,
                    a: (0.90 * opacity).clamp(0.2, 1.0),
                },
                None,
            )?;
            let secondary_text_brush = target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.70,
                    g: 0.80,
                    b: 0.92,
                    a: (0.85 * opacity).clamp(0.2, 1.0),
                },
                None,
            )?;
            let speaker_bg_brush = target.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.15,
                    g: 0.28,
                    b: 0.45,
                    a: (0.85 * opacity).clamp(0.2, 1.0),
                },
                None,
            )?;

            let padding_x = 20.0f32;
            let card_width = (self.width as f32 - padding_x * 2.0).max(100.0);
            let layout_max_width = (card_width - 32.0).max(64.0);
            let layout_max_height = self.height as f32;
            let mut curr_y = 16.0f32;

            for card in cards {
                let has_secondary = bilingual
                    && !card.source.trim().is_empty()
                    && card.source.trim() != card.translated.trim();

                let primary_text = if card.translated.trim().is_empty() {
                    if card.source.trim().is_empty() {
                        " "
                    } else {
                        &card.source
                    }
                } else {
                    &card.translated
                };

                let mut p_u16: Vec<u16> = primary_text.encode_utf16().collect();
                if p_u16.is_empty() {
                    p_u16 = vec![0x0020];
                }
                let p_layout = dwrite.CreateTextLayout(
                    &p_u16,
                    &primary_format,
                    layout_max_width,
                    layout_max_height,
                )?;
                let mut p_metrics = DWRITE_TEXT_METRICS::default();
                p_layout.GetMetrics(&mut p_metrics)?;

                let mut card_height = p_metrics.height + 20.0;
                let mut s_layout_opt = None;

                if has_secondary {
                    let mut s_u16: Vec<u16> = card.source.encode_utf16().collect();
                    if s_u16.is_empty() {
                        s_u16 = vec![0x0020];
                    }
                    let s_layout = dwrite.CreateTextLayout(
                        &s_u16,
                        &secondary_format,
                        layout_max_width,
                        layout_max_height,
                    )?;
                    let mut s_metrics = DWRITE_TEXT_METRICS::default();
                    s_layout.GetMetrics(&mut s_metrics)?;
                    card_height += s_metrics.height + 4.0;
                    s_layout_opt = Some((s_layout, s_metrics.height));
                }

                if !card.speaker.is_empty() {
                    card_height += 18.0;
                }

                // Draw card container
                let card_rect = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: padding_x,
                        top: curr_y,
                        right: padding_x + card_width,
                        bottom: curr_y + card_height,
                    },
                    radiusX: 8.0,
                    radiusY: 8.0,
                };
                target.FillRoundedRectangle(
                    &card_rect,
                    if card.live {
                        &card_bg
                    } else {
                        &history_card_bg
                    },
                );
                target.DrawRoundedRectangle(
                    &card_rect,
                    if card.live {
                        &live_border_brush
                    } else {
                        &border_brush
                    },
                    if card.live { 1.8 } else { 1.0 },
                    None,
                );

                let mut text_y = curr_y + 10.0;

                // Live status dot / speaker header
                if card.live {
                    let dot_rect = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                        point: D2D_POINT_2F {
                            x: padding_x + 10.0,
                            y: text_y + 8.0,
                        },
                        radiusX: 3.5,
                        radiusY: 3.5,
                    };
                    target.FillEllipse(&dot_rect, &live_dot_brush);
                }

                // Speaker badge
                if !card.speaker.is_empty() {
                    let spk_u16: Vec<u16> = card.speaker.encode_utf16().collect();
                    if let Ok(spk_layout) =
                        dwrite.CreateTextLayout(&spk_u16, &speaker_format, 200.0, 24.0)
                    {
                        let mut spk_metrics = DWRITE_TEXT_METRICS::default();
                        let _ = spk_layout.GetMetrics(&mut spk_metrics);
                        let badge_w = spk_metrics.width + 14.0;
                        let badge_rect = D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: padding_x + 16.0,
                                top: text_y,
                                right: padding_x + 16.0 + badge_w,
                                bottom: text_y + 16.0,
                            },
                            radiusX: 4.0,
                            radiusY: 4.0,
                        };
                        target.FillRoundedRectangle(&badge_rect, &speaker_bg_brush);
                        target.DrawTextLayout(
                            D2D_POINT_2F {
                                x: padding_x + 23.0,
                                y: text_y + 1.0,
                            },
                            &spk_layout,
                            &primary_live_brush,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                        );
                        text_y += 18.0;
                    }
                }

                // Primary Translated Text
                let text_x = if card.live {
                    padding_x + 18.0
                } else {
                    padding_x + 16.0
                };
                target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: text_x,
                        y: text_y,
                    },
                    &p_layout,
                    if card.live {
                        &primary_live_brush
                    } else {
                        &primary_hist_brush
                    },
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
                text_y += p_metrics.height + 4.0;

                // Secondary Source Text
                if let Some((s_layout, _)) = s_layout_opt {
                    target.DrawTextLayout(
                        D2D_POINT_2F {
                            x: text_x,
                            y: text_y,
                        },
                        &s_layout,
                        &secondary_text_brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                }

                curr_y += card_height + 8.0;
                if curr_y >= self.height as f32 - 30.0 {
                    break;
                }
            }

            target.EndDraw(None, None)?;

            // Copy DIB pixels into output buffer (RGBA conversion)
            if !dib_bits.is_null() {
                let src_slice = std::slice::from_raw_parts(
                    dib_bits as *const u8,
                    (self.width * self.height * 4) as usize,
                );
                // Windows DIB is BGRA premultiplied; convert to RGBA
                for (src, dst) in src_slice.chunks_exact(4).zip(out_buffer.chunks_exact_mut(4)) {
                    dst[0] = src[2]; // R
                    dst[1] = src[1]; // G
                    dst[2] = src[0]; // B
                    dst[3] = src[3]; // A
                }
            }

            Ok(())
        }
    }

    fn render_fallback(&self, _cards: &[VrSubtitleCard], out_buffer: &mut [u8]) {
        out_buffer.fill(0);
    }
}
