#[cfg(target_os = "linux")]
mod linux {
    use std::{f64::consts::PI, thread, time::Duration};

    use gtk::{
        cairo::{Context, FontSlant, FontWeight, LineCap, Operator},
        gdk,
        glib::Propagation,
        prelude::{BinExt, ContainerExt, GtkWindowExt, OverlayExt, WidgetExt, WidgetExtManual},
        DrawingArea, Overlay,
    };
    use tauri::{AppHandle, Manager, WebviewWindow};

    use crate::{
        model::{Theme, WindowDisplay},
        providers::ProviderStore,
        render::{self, ProviderView, WidgetView, WindowView},
        settings::SettingsStore,
        window::{ManagedWindowState, WIDGET_LABEL},
    };

    type Color = (f64, f64, f64, f64);

    /// How many lines of a provider warning the card will show.
    ///
    /// Two covers every message the providers actually produce; the cap exists
    /// for the one that embeds an arbitrary network error and so has no length
    /// of its own. See [`wrap_lines`].
    const WARNING_MAX_LINES: usize = 2;

    /// Space above the warning, separating it from the metrics.
    const WARNING_GAP: f64 = 10.0;

    #[derive(Clone, Copy)]
    struct Palette {
        surface: Color,
        card: Color,
        text: Color,
        muted: Color,
        faint: Color,
        line: Color,
        track: Color,
        accent: Color,
        error: Color,
    }

    pub fn install(app: &AppHandle, window: &WebviewWindow) -> Result<(), String> {
        let gtk_window = window
            .gtk_window()
            .map_err(|error| format!("could not access GTK widget window: {error}"))?;
        let webview = gtk_window
            .child()
            .ok_or_else(|| "widget window has no webview child".to_string())?;
        gtk_window.remove(&webview);

        let overlay = Overlay::new();
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);
        overlay.add(&webview);

        let canvas = DrawingArea::new();
        canvas.set_hexpand(true);
        canvas.set_vexpand(true);
        canvas.set_can_focus(false);
        canvas.add_events(
            gdk::EventMask::BUTTON_PRESS_MASK
                | gdk::EventMask::POINTER_MOTION_MASK
                | gdk::EventMask::LEAVE_NOTIFY_MASK,
        );
        overlay.add_overlay(&canvas);
        gtk_window.add(&overlay);
        overlay.show_all();
        webview.hide();

        let draw_app = app.clone();
        canvas.connect_draw(move |area, context| {
            draw(&draw_app, area, context);
            Propagation::Proceed
        });

        let input_app = app.clone();
        let input_window = gtk_window.clone();
        canvas.connect_button_press_event(move |area, event| {
            if input_app.state::<ManagedWindowState>().snapshot().locked || event.button() != 1 {
                return Propagation::Proceed;
            }
            let allocation = area.allocation();
            let (x, y) = event.position();
            let (root_x, root_y) = event.root();
            if let Some(edge) = resize_edge(x, y, allocation.width(), allocation.height()) {
                input_window.begin_resize_drag(
                    edge,
                    event.button() as i32,
                    root_x as i32,
                    root_y as i32,
                    event.time(),
                );
            } else {
                input_window.begin_move_drag(
                    event.button() as i32,
                    root_x as i32,
                    root_y as i32,
                    event.time(),
                );
            }
            Propagation::Stop
        });

        let tick_app = app.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(30));
            super::redraw(&tick_app);
        });
        Ok(())
    }

    pub fn set_locked(app: &AppHandle, locked: bool) {
        let Some(window) = app.get_webview_window(WIDGET_LABEL) else {
            return;
        };
        let Ok(gtk_window) = window.gtk_window() else {
            return;
        };
        if locked {
            let region =
                gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(0, 0, 1, 1));
            gtk_window.input_shape_combine_region(Some(&region));
        } else {
            gtk_window.input_shape_combine_region(None);
        }
    }

    pub fn redraw(app: &AppHandle) {
        let app_for_main = app.clone();
        let _ = app.run_on_main_thread(move || {
            let Some(window) = app_for_main.get_webview_window(WIDGET_LABEL) else {
                return;
            };
            if let Ok(gtk_window) = window.gtk_window() {
                gtk_window.queue_draw();
            }
        });
    }

    fn draw(app: &AppHandle, area: &DrawingArea, context: &Context) {
        let allocation = area.allocation();
        let width = f64::from(allocation.width());
        let height = f64::from(allocation.height());
        if width < 2.0 || height < 2.0 {
            return;
        }

        // Every decision about what the widget says is made in `render`, which
        // the Windows renderer reads too. This function only paints.
        let settings = app.state::<SettingsStore>().snapshot();
        let view = render::widget_view(
            settings.theme,
            &settings.provider_order,
            &settings.disabled_providers,
            app.state::<ProviderStore>().snapshots(),
            &app.state::<ManagedWindowState>().snapshot(),
            crate::providers::now_unix(),
        );
        let colors = palette(view.theme);

        context.set_operator(Operator::Source);
        context.set_source_rgba(0.0, 0.0, 0.0, 0.0);
        let _ = context.paint();
        context.set_operator(Operator::Over);

        rounded_rect(context, 10.0, 10.0, width - 20.0, height - 20.0, 21.0);
        rgba(context, colors.surface);
        let _ = context.fill_preserve();
        rgba(
            context,
            if view.locked {
                colors.line
            } else {
                colors.accent
            },
        );
        context.set_line_width(if view.locked { 1.0 } else { 2.0 });
        let _ = context.stroke();
        if !view.locked {
            draw_editing_handles(context, width, height, colors.accent);
        }

        label(
            context,
            "AGENT GAUGE",
            26.0,
            33.0,
            11.0,
            FontWeight::Bold,
            colors.faint,
        );
        label_right(
            context,
            &view.mode_label,
            width - 26.0,
            33.0,
            10.5,
            FontWeight::Bold,
            colors.faint,
        );

        draw_provider_grid(context, &view, width, height, colors);
        if !view.locked {
            label_center(
                context,
                "LAYOUT UNLOCKED · DRAG OR RESIZE",
                width / 2.0,
                height - 14.0,
                7.0,
                FontWeight::Bold,
                colors.accent,
            );
        }
    }

    fn draw_provider_grid(
        context: &Context,
        view: &WidgetView,
        width: f64,
        height: f64,
        colors: Palette,
    ) {
        if let Some(empty) = &view.empty {
            label_center(
                context,
                &empty.title,
                width / 2.0,
                height / 2.0,
                13.0,
                FontWeight::Bold,
                colors.text,
            );
            label_center(
                context,
                &empty.detail,
                width / 2.0,
                height / 2.0 + 19.0,
                9.0,
                FontWeight::Normal,
                colors.muted,
            );
            return;
        }
        let providers = &view.providers;

        let x = 20.0;
        let y = 47.0;
        let available_width = width - 40.0;
        let available_height = height - 70.0;
        let columns: usize = if width >= 620.0 { 2 } else { 1 };
        let rows = providers.len().div_ceil(columns);
        let gap = 10.0;
        let card_width =
            (available_width - gap * columns.saturating_sub(1) as f64) / columns as f64;

        if columns == 1 {
            let width_scale = (card_width / 380.0).clamp(0.9, 1.12);
            let base_total = providers
                .iter()
                .map(|provider| provider_desired_height(provider, width_scale))
                .sum::<f64>()
                + gap * providers.len().saturating_sub(1) as f64;
            let fit_scale = if base_total > available_height {
                (available_height / base_total).clamp(0.86, 1.0)
            } else {
                1.0
            };
            let scale = width_scale * fit_scale;
            let mut heights: Vec<_> = providers
                .iter()
                .map(|provider| provider_desired_height(provider, scale))
                .collect();
            let natural_total =
                heights.iter().sum::<f64>() + gap * providers.len().saturating_sub(1) as f64;
            let growth = ((available_height - natural_total).max(0.0)
                / providers.len().max(1) as f64)
                .min(28.0 * scale);
            for card_height in &mut heights {
                *card_height += growth;
            }
            let content_height =
                heights.iter().sum::<f64>() + gap * providers.len().saturating_sub(1) as f64;
            let mut card_y = y + (available_height - content_height).max(0.0) / 2.0;
            for (provider, card_height) in providers.iter().zip(heights) {
                draw_provider(
                    context,
                    provider,
                    x,
                    card_y,
                    card_width,
                    card_height,
                    scale,
                    colors,
                );
                card_y += card_height + gap;
            }
            return;
        }

        let card_height = (available_height - gap * rows.saturating_sub(1) as f64) / rows as f64;
        let scale = (card_width / 330.0)
            .min(card_height / 185.0)
            .clamp(0.88, 1.12);
        for (index, provider) in providers.iter().enumerate() {
            let column = index % columns;
            let row = index / columns;
            draw_provider(
                context,
                provider,
                x + column as f64 * (card_width + gap),
                y + row as f64 * (card_height + gap),
                card_width,
                card_height,
                scale,
                colors,
            );
        }
    }

    fn provider_desired_height(provider: &ProviderView, scale: f64) -> f64 {
        if provider.windows.is_empty() {
            return 112.0 * scale;
        }
        let ring_height = if provider
            .windows
            .iter()
            .any(|window| window.display == WindowDisplay::Ring)
        {
            82.0
        } else {
            0.0
        };
        let bars = provider
            .windows
            .iter()
            .filter(|window| window.display == WindowDisplay::Bar)
            .count() as f64;
        let balance_height = if provider.balances.is_empty() {
            0.0
        } else {
            29.0
        };
        (54.0 + ring_height + bars * 44.0 + balance_height) * scale
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_provider(
        context: &Context,
        provider: &ProviderView,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        scale: f64,
        colors: Palette,
    ) {
        let text_scale = (scale * 1.3).clamp(1.16, 1.34);
        rounded_rect(context, x, y, width, height, 14.0);
        rgba(context, colors.card);
        let _ = context.fill_preserve();
        rgba(context, colors.line);
        context.set_line_width(1.0);
        let _ = context.stroke();

        let accent = provider
            .accent
            .as_deref()
            .and_then(parse_hex)
            .unwrap_or(colors.accent);
        context.new_sub_path();
        context.arc(
            x + 17.0 * scale,
            y + 20.0 * scale,
            3.8 * scale,
            0.0,
            PI * 2.0,
        );
        rgba(context, accent);
        let _ = context.fill();
        label(
            context,
            &provider.name,
            x + 29.0 * scale,
            y + 24.5 * scale,
            13.0 * text_scale,
            FontWeight::Bold,
            colors.text,
        );
        label_right(
            context,
            &provider.status_label,
            x + width - 14.0 * scale,
            y + 23.5 * scale,
            9.5 * text_scale,
            FontWeight::Normal,
            if provider.warning.is_some() {
                colors.error
            } else {
                colors.muted
            },
        );

        if let Some(notice) = &provider.notice {
            label(
                context,
                &notice.title,
                x + 16.0 * scale,
                y + 62.0 * scale,
                12.0 * text_scale,
                FontWeight::Bold,
                colors.text,
            );
            label_wrapped(
                context,
                &notice.detail,
                x + 16.0 * scale,
                y + 83.0 * scale,
                width - 32.0 * scale,
                9.5 * text_scale,
                colors.muted,
            );
            return;
        }

        let ring = provider
            .windows
            .iter()
            .find(|window| window.display == WindowDisplay::Ring);
        let bars: Vec<_> = provider
            .windows
            .iter()
            .filter(|window| window.display == WindowDisplay::Bar)
            .collect();
        let has_balance = !provider.balances.is_empty();

        // Measured before anything is positioned: a warning takes room from the
        // same centred block as the metrics, so it has to be part of the height
        // they are centred by. Drawing it afterwards without reserving the space
        // would overlap whatever sits above it.
        let warning_size = 9.0 * text_scale;
        let warning_lines = provider
            .warning
            .as_deref()
            .map(|warning| {
                wrap_lines(
                    context,
                    warning,
                    width - 32.0 * scale,
                    warning_size,
                    WARNING_MAX_LINES,
                )
            })
            .unwrap_or_default();

        let ring_block = if ring.is_some() { 82.0 * scale } else { 0.0 };
        let bar_block = bars.len() as f64 * 44.0 * scale;
        let balance_block = if has_balance { 29.0 * scale } else { 0.0 };
        let warning_block = if warning_lines.is_empty() {
            0.0
        } else {
            WARNING_GAP * scale + warning_lines.len() as f64 * leading(warning_size)
        };
        let block_height = ring_block + bar_block + balance_block + warning_block;
        let content_top = y + 43.0 * scale;
        let content_bottom = y + height - 11.0 * scale;
        let mut cursor = content_top + (content_bottom - content_top - block_height).max(0.0) / 2.0;
        if let Some(ring) = ring {
            let radius = (31.0 * scale).min(width * 0.16);
            let center_x = x + radius + 17.0 * scale;
            let center_y = cursor + 37.0 * scale;
            draw_ring(
                context, ring, center_x, center_y, radius, scale, text_scale, accent, colors,
            );

            let copy_x = center_x + radius + 16.0 * scale;
            label(
                context,
                &ring.label.to_uppercase(),
                copy_x,
                center_y - 20.0 * scale,
                9.5 * text_scale,
                FontWeight::Bold,
                accent,
            );
            label(
                context,
                &ring.primary,
                copy_x,
                center_y + 1.0 * scale,
                12.0 * text_scale,
                FontWeight::Bold,
                colors.text,
            );
            label(
                context,
                &ring.secondary,
                copy_x,
                center_y + 20.0 * scale,
                9.5 * text_scale,
                FontWeight::Normal,
                colors.muted,
            );
            cursor += ring_block;
        }
        for window in bars {
            draw_bar(
                context,
                window,
                x + 16.0 * scale,
                cursor + 4.0 * scale,
                width - 32.0 * scale,
                scale,
                text_scale,
                accent,
                colors,
            );
            cursor += 44.0 * scale;
        }

        if let Some(balance) = provider.balances.first() {
            let amount = &balance.amount;
            context.move_to(x + 16.0 * scale, cursor + 4.0 * scale);
            context.line_to(x + width - 16.0 * scale, cursor + 4.0 * scale);
            rgba(context, colors.line);
            context.set_line_width(1.0);
            let _ = context.stroke();
            label(
                context,
                &balance.label,
                x + 16.0 * scale,
                cursor + 22.0 * scale,
                9.5 * text_scale,
                FontWeight::Normal,
                colors.muted,
            );
            label_right(
                context,
                amount,
                x + width - 16.0 * scale,
                cursor + 22.0 * scale,
                10.5 * text_scale,
                FontWeight::Bold,
                colors.text,
            );
            cursor += balance_block;
        }

        // The reason a reading has stopped moving, in the provider's own words.
        // Previously this only tinted the status label, which named the state
        // ("Stale") but never the cause — leaving the one sentence that says
        // what to do about it visible in Settings and nowhere else.
        for (index, line) in warning_lines.iter().enumerate() {
            label(
                context,
                line,
                x + 16.0 * scale,
                cursor + WARNING_GAP * scale + warning_size + index as f64 * leading(warning_size),
                warning_size,
                FontWeight::Normal,
                colors.error,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_ring(
        context: &Context,
        window: &WindowView,
        x: f64,
        y: f64,
        radius: f64,
        scale: f64,
        text_scale: f64,
        accent: Color,
        colors: Palette,
    ) {
        context.set_line_width(7.5 * scale);
        context.new_sub_path();
        context.arc(x, y, radius, -PI / 2.0, PI * 1.5);
        rgba(context, colors.track);
        let _ = context.stroke();
        context.new_sub_path();
        context.arc(
            x,
            y,
            radius,
            -PI / 2.0,
            -PI / 2.0 + PI * 2.0 * window.fill / 100.0,
        );
        rgba(context, accent);
        context.set_line_cap(LineCap::Round);
        let _ = context.stroke();
        context.set_line_cap(LineCap::Butt);
        label_center(
            context,
            &window.percent_label,
            x,
            y + 2.0 * scale,
            15.0 * text_scale,
            FontWeight::Bold,
            colors.text,
        );
        label_center(
            context,
            "USED",
            x,
            y + 16.0 * scale,
            7.0 * text_scale,
            FontWeight::Bold,
            colors.muted,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_bar(
        context: &Context,
        window: &WindowView,
        x: f64,
        y: f64,
        width: f64,
        scale: f64,
        text_scale: f64,
        accent: Color,
        colors: Palette,
    ) {
        label(
            context,
            &window.label,
            x,
            y,
            10.0 * text_scale,
            FontWeight::Normal,
            colors.muted,
        );
        label_right(
            context,
            &format!("{} used", window.percent_label),
            x + width,
            y,
            10.0 * text_scale,
            FontWeight::Bold,
            colors.text,
        );
        rounded_rect(
            context,
            x,
            y + 11.0 * scale,
            width,
            8.0 * scale,
            4.0 * scale,
        );
        rgba(context, colors.track);
        let _ = context.fill();
        rounded_rect(
            context,
            x,
            y + 11.0 * scale,
            width * window.fill / 100.0,
            8.0 * scale,
            4.0 * scale,
        );
        rgba(context, accent);
        let _ = context.fill();
        label(
            context,
            &format!("{} · {}", window.primary, window.secondary),
            x,
            y + 31.0 * scale,
            9.0 * text_scale,
            FontWeight::Normal,
            colors.faint,
        );
    }

    fn resize_edge(x: f64, y: f64, width: i32, height: i32) -> Option<gdk::WindowEdge> {
        let edge = 22.0;
        let left = x <= edge;
        let right = x >= f64::from(width) - edge;
        let top = y <= edge;
        let bottom = y >= f64::from(height) - edge;
        match (left, right, top, bottom) {
            (true, _, true, _) => Some(gdk::WindowEdge::NorthWest),
            (_, true, true, _) => Some(gdk::WindowEdge::NorthEast),
            (true, _, _, true) => Some(gdk::WindowEdge::SouthWest),
            (_, true, _, true) => Some(gdk::WindowEdge::SouthEast),
            (_, _, true, _) => Some(gdk::WindowEdge::North),
            (_, true, _, _) => Some(gdk::WindowEdge::East),
            (_, _, _, true) => Some(gdk::WindowEdge::South),
            (true, _, _, _) => Some(gdk::WindowEdge::West),
            _ => None,
        }
    }

    fn draw_editing_handles(context: &Context, width: f64, height: f64, accent: Color) {
        let inset = 11.0;
        let points = [
            (inset, inset),
            (width / 2.0, inset),
            (width - inset, inset),
            (width - inset, height / 2.0),
            (width - inset, height - inset),
            (width / 2.0, height - inset),
            (inset, height - inset),
            (inset, height / 2.0),
        ];
        rgba(context, accent);
        for (x, y) in points {
            context.new_sub_path();
            context.arc(x, y, 3.25, 0.0, PI * 2.0);
            let _ = context.fill();
        }
    }

    fn palette(theme: Theme) -> Palette {
        match theme {
            Theme::Signal => Palette {
                surface: (0.09, 0.10, 0.10, 0.98),
                card: (0.125, 0.14, 0.135, 0.98),
                text: (0.95, 0.93, 0.89, 1.0),
                muted: (0.61, 0.60, 0.57, 1.0),
                faint: (0.43, 0.44, 0.42, 1.0),
                line: (0.95, 0.93, 0.89, 0.11),
                track: (0.95, 0.93, 0.89, 0.10),
                accent: (0.91, 0.65, 0.36, 1.0),
                error: (0.91, 0.51, 0.47, 1.0),
            },
            Theme::Glass => Palette {
                surface: (0.08, 0.10, 0.105, 0.78),
                card: (0.15, 0.17, 0.17, 0.66),
                text: (0.96, 0.96, 0.93, 1.0),
                muted: (0.72, 0.73, 0.70, 1.0),
                faint: (0.52, 0.54, 0.52, 1.0),
                line: (1.0, 1.0, 1.0, 0.15),
                track: (1.0, 1.0, 1.0, 0.11),
                accent: (0.91, 0.65, 0.36, 1.0),
                error: (0.94, 0.55, 0.50, 1.0),
            },
            Theme::Cutout => Palette {
                surface: (0.03, 0.035, 0.035, 0.28),
                card: (0.03, 0.035, 0.035, 0.32),
                text: (0.97, 0.96, 0.93, 1.0),
                muted: (0.82, 0.81, 0.78, 1.0),
                faint: (0.67, 0.66, 0.63, 1.0),
                line: (1.0, 1.0, 1.0, 0.14),
                track: (1.0, 1.0, 1.0, 0.14),
                accent: (0.86, 0.86, 0.83, 1.0),
                error: (0.96, 0.62, 0.57, 1.0),
            },
        }
    }

    fn parse_hex(value: &str) -> Option<Color> {
        let value = value.strip_prefix('#')?;
        if value.len() != 6 {
            return None;
        }
        let red = u8::from_str_radix(&value[0..2], 16).ok()?;
        let green = u8::from_str_radix(&value[2..4], 16).ok()?;
        let blue = u8::from_str_radix(&value[4..6], 16).ok()?;
        Some((
            f64::from(red) / 255.0,
            f64::from(green) / 255.0,
            f64::from(blue) / 255.0,
            1.0,
        ))
    }

    fn rounded_rect(context: &Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let radius = radius.min(width / 2.0).min(height / 2.0);
        context.new_sub_path();
        context.arc(x + width - radius, y + radius, radius, -PI / 2.0, 0.0);
        context.arc(
            x + width - radius,
            y + height - radius,
            radius,
            0.0,
            PI / 2.0,
        );
        context.arc(x + radius, y + height - radius, radius, PI / 2.0, PI);
        context.arc(x + radius, y + radius, radius, PI, PI * 1.5);
        context.close_path();
    }

    fn rgba(context: &Context, color: Color) {
        context.set_source_rgba(color.0, color.1, color.2, color.3);
    }

    #[allow(clippy::too_many_arguments)]
    fn label(
        context: &Context,
        value: &str,
        x: f64,
        y: f64,
        size: f64,
        weight: FontWeight,
        color: Color,
    ) {
        context.select_font_face("Sans", FontSlant::Normal, weight);
        context.set_font_size(size);
        rgba(context, color);
        context.move_to(x, y);
        let _ = context.show_text(value);
    }

    #[allow(clippy::too_many_arguments)]
    fn label_right(
        context: &Context,
        value: &str,
        right: f64,
        y: f64,
        size: f64,
        weight: FontWeight,
        color: Color,
    ) {
        context.select_font_face("Sans", FontSlant::Normal, weight);
        context.set_font_size(size);
        let width = context
            .text_extents(value)
            .map(|extents| extents.x_advance())
            .unwrap_or(0.0);
        label(context, value, right - width, y, size, weight, color);
    }

    #[allow(clippy::too_many_arguments)]
    fn label_center(
        context: &Context,
        value: &str,
        center: f64,
        y: f64,
        size: f64,
        weight: FontWeight,
        color: Color,
    ) {
        context.select_font_face("Sans", FontSlant::Normal, weight);
        context.set_font_size(size);
        let width = context
            .text_extents(value)
            .map(|extents| extents.x_advance())
            .unwrap_or(0.0);
        label(context, value, center - width / 2.0, y, size, weight, color);
    }

    #[allow(clippy::too_many_arguments)]
    fn label_wrapped(
        context: &Context,
        value: &str,
        x: f64,
        y: f64,
        max_width: f64,
        size: f64,
        color: Color,
    ) {
        for (index, line) in wrap_lines(context, value, max_width, size, usize::MAX)
            .iter()
            .enumerate()
        {
            let line_y = y + index as f64 * leading(size);
            label(context, line, x, line_y, size, FontWeight::Normal, color);
        }
    }

    /// The baseline-to-baseline distance for wrapped text at `size`.
    ///
    /// Shared so that code reserving vertical space and code drawing into it
    /// cannot disagree about how tall the result is.
    fn leading(size: f64) -> f64 {
        size + 4.0
    }

    /// Breaks `value` into lines that fit `max_width`, to at most `max_lines`.
    ///
    /// Returning the lines rather than drawing them is what lets a caller
    /// measure a block before laying it out. When the text does not fit in
    /// `max_lines` the final line is ellipsized, so a message cut short reads
    /// as cut short instead of simply ending mid-sentence — the failure
    /// messages this renders include one that embeds an arbitrary network
    /// error, so some bound is required.
    fn wrap_lines(
        context: &Context,
        value: &str,
        max_width: f64,
        size: f64,
        max_lines: usize,
    ) -> Vec<String> {
        context.select_font_face("Sans", FontSlant::Normal, FontWeight::Normal);
        context.set_font_size(size);
        let measure = |text: &str| {
            context
                .text_extents(text)
                .map(|extents| extents.x_advance())
                .unwrap_or(0.0)
        };

        let mut lines: Vec<String> = Vec::new();
        let mut line = String::new();
        for word in value.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if measure(&candidate) > max_width && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                if lines.len() == max_lines {
                    if let Some(last) = lines.last_mut() {
                        *last = ellipsize(&measure, last, max_width);
                    }
                    return lines;
                }
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines
    }

    /// Appends an ellipsis, dropping trailing words until it fits.
    ///
    /// The last word always survives even if it overflows, so the result is
    /// never an ellipsis on its own.
    fn ellipsize(measure: &impl Fn(&str) -> f64, line: &str, max_width: f64) -> String {
        let mut words: Vec<&str> = line.split_whitespace().collect();
        loop {
            let candidate = format!("{}…", words.join(" "));
            if words.len() <= 1 || measure(&candidate) <= max_width {
                return candidate;
            }
            words.pop();
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;

        /// Text measurement needs a Cairo context, but not a display: an
        /// off-screen surface measures identically to the widget's own.
        fn context() -> Context {
            let surface =
                gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, 400, 200).unwrap();
            Context::new(&surface).unwrap()
        }

        #[test]
        fn short_text_stays_on_one_line() {
            let lines = wrap_lines(&context(), "Stale", 300.0, 9.0, WARNING_MAX_LINES);

            assert_eq!(lines, vec!["Stale".to_string()]);
        }

        #[test]
        fn text_is_wrapped_within_the_width_it_is_given() {
            let context = context();
            let lines = wrap_lines(
                &context,
                "Claude sign-in has expired; open Claude Code once to refresh it",
                120.0,
                9.0,
                WARNING_MAX_LINES,
            );

            context.set_font_size(9.0);
            for line in &lines {
                let width = context.text_extents(line).unwrap().x_advance();
                assert!(width <= 120.0, "{line:?} overflows at {width}");
            }
        }

        #[test]
        fn an_overlong_message_is_capped_and_marked_as_cut_short() {
            // The bound that keeps a card's layout predictable: one failure
            // message embeds an arbitrary network error and has no length of
            // its own.
            let lines = wrap_lines(
                &context(),
                "Could not reach Claude for usage: dns error: failed to look up \
                 the address information for the host after several attempts",
                120.0,
                9.0,
                WARNING_MAX_LINES,
            );

            assert_eq!(lines.len(), WARNING_MAX_LINES);
            assert!(
                lines.last().expect("a final line").ends_with('…'),
                "a truncated message should read as truncated: {lines:?}"
            );
        }

        #[test]
        fn a_single_word_too_long_to_fit_is_kept_rather_than_reduced_to_dots() {
            let lines = wrap_lines(
                &context(),
                "https://api.anthropic.com/api/oauth/usage/an/extremely/long/path",
                40.0,
                9.0,
                1,
            );

            assert_eq!(lines.len(), 1);
            assert!(lines[0].len() > 1, "{lines:?}");
        }

        #[test]
        fn reserved_height_matches_what_the_lines_will_occupy() {
            // The invariant the warning block depends on: measure and draw
            // must agree, or the text lands on top of the metrics.
            assert_eq!(leading(9.0), 13.0);
            assert_eq!(leading(10.0), 14.0);
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{install, redraw, set_locked};

#[cfg(not(target_os = "linux"))]
pub fn install(_app: &tauri::AppHandle, _window: &tauri::WebviewWindow) -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn redraw(_app: &tauri::AppHandle) {}

#[cfg(not(target_os = "linux"))]
pub fn set_locked(_app: &tauri::AppHandle, _locked: bool) {}
