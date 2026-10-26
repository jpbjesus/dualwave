//! Exercise the real page widgets without starting controller or audio workers.
use super::*;

#[test]
fn editor_pages_stay_inside_the_content_column() {
    let dir = std::env::temp_dir().join(format!("dualwave-editor-layout-{}", std::process::id()));
    let mut app = App::new(Engine::new(dir.clone()));
    let ctx = egui::Context::default();
    theme::install(&ctx, 1.0);
    let mut failures = Vec::new();
    for size in [egui::vec2(1240.0, 800.0), egui::vec2(1024.0, 640.0), egui::vec2(760.0, 520.0)] {
        for page in Page::ALL {
            app.page = page;
            for expanded in [false, true] {
                let mut profile = Profile::default();
                if expanded {
                    profile.gyro.mode = crate::profile::GyroMode::Mouse;
                    profile.touchpad.mode = crate::profile::TouchMode::Mouse;
                    profile.sticks.left.mode = crate::profile::StickMode::Mouse;
                    profile.sticks.right.mode = crate::profile::StickMode::Wasd;
                    profile.haptics.source = crate::profile::AudioSource::SystemAudio;
                    profile.haptics.buttons.enabled = true;
                    profile.haptics.triggers.enabled = true;
                    profile.virtual_out.kind = crate::profile::VirtualKind::DualSense;
                    profile.triggers.left = ds_proto::TriggerEffect::Feedback { position: 1, strength: 5 };
                    profile.triggers.right = ds_proto::TriggerEffect::Weapon { start: 3, end: 6, strength: 6 };
                    profile.mappings.push(crate::profile::Mapping::default());
                }
                for pass in 0..3 {
                    let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() }, |ctx| {
                        egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 8))).show(ctx, |ui| {
                            let right = ui.available_rect_before_wrap().right();
                            egui::ScrollArea::vertical().id_salt(("page-layout", format!("{page:?}"), expanded)).auto_shrink([false, false]).show(ui, |ui| {
                                match page {
                                    Page::Overview => app.page_overview(ui, &mut profile, None),
                                    Page::Lighting => app.page_lighting(ui, &mut profile, None),
                                    Page::Triggers => app.page_triggers(ui, &mut profile, None),
                                    Page::Haptics => app.page_haptics(ui, &mut profile, None),
                                    Page::Input => app.page_input(ui, &mut profile, None),
                                    Page::Mappings => app.page_mappings(ui, ctx, &mut profile),
                                    Page::Virtual => app.page_virtual(ui, &mut profile, None),
                                    Page::Profiles => app.page_profiles(ui, &mut profile, None),
                                    Page::Settings => app.page_settings(ui),
                                }
                                if pass == 2 && ui.min_rect().right() > right + 1.0 {
                                    failures.push(format!("{page:?} expanded={expanded} at {size:?}: right={} limit={right}", ui.min_rect().right()));
                                }
                            });
                        });
                    });
                }
            }
        }
    }
    drop(app);
    let _ = std::fs::remove_dir_all(dir);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn segmented_controls_wrap_long_translations_inside_their_column() {
    let ctx = egui::Context::default();
    theme::install(&ctx, 1.0);
    for width in [300.0, 460.0] {
        let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 400.0))), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let available = ui.available_rect_before_wrap();
                let mut value = 0;
                widgets::segmented(ui, &mut value, &[(0, "Fixa"), (1, "Arco-íris"), (2, "Pulsação suave"), (3, "Estrobo"), (4, "Bateria"), (5, "Desligado")]);
                assert!(ui.min_rect().right() <= available.right() + 1.0, "translated segments exceed {width} pt");
            });
        });
    }
}

#[test]
fn setting_help_has_clear_spacing_and_uses_room_beside_a_toggle() {
    let ctx = egui::Context::default();
    theme::install(&ctx, 1.0);
    let title = "Manter ligado com o comando desligado";
    let help = "Mantém o comando virtual ligado enquanto o comando físico está desligado, para o jogo conservar o mesmo dispositivo.";
    let output = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(580.0, 300.0))), ..Default::default() }, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut enabled = true;
            widgets::toggle_row(ui, title, help, &mut enabled);
        });
    });
    let text = |value: &str| output.shapes.iter().find_map(|s| match &s.shape {
        egui::Shape::Text(t) if t.galley.job.text == value => Some(t), _ => None,
    }).expect("setting text rendered");
    let label = text(title);
    let hint = text(help);
    assert!(hint.pos.y >= label.pos.y + label.galley.size().y + 3.0, "setting title and help are cramped together");
    assert!(hint.galley.rows.len() <= 2, "toggle help is squeezed into a narrow label column");
}

#[test]
fn session_header_keeps_long_profile_names_inside_the_window() {
    let dir = std::env::temp_dir().join(format!("dualwave-header-layout-{}", std::process::id()));
    let engine = Engine::new(dir.clone());
    let name = "Perfil personalizado para jogos com auscultadores e gatilhos adaptativos";
    let name = engine.profiles.copy_as("Xbox", name);
    engine.settings.write().default_profile = name;
    let mut app = App::new(engine);
    let ctx = egui::Context::default();
    theme::install(&ctx, 1.0);
    for pass in 0..3 {
        let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(760.0, 520.0))), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 8))).show(ctx, |ui| {
                let available = ui.available_rect_before_wrap();
                app.top_bar(ui, None);
                if pass == 2 { assert!(ui.min_rect().right() <= available.right() + 1.0, "long profile name pushes the session header outside the viewport"); }
            });
        });
    }
    drop(app);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn profiles_keep_navigation_visible_with_long_lists_and_game_rules() {
    let dir = std::env::temp_dir().join(format!("dualwave-profiles-layout-{}", std::process::id()));
    let engine = Engine::new(dir.clone());
    for n in 0..8 { engine.profiles.copy_as("Xbox", &format!("Profile {n} for controller and audio")); }
    engine.settings.write().games.push(crate::settings::GameRule { enabled: true, exe: "example-game.exe".into(), profile: "A profile that is no longer present".into() });
    engine.game_status.write().games.push(crate::games::DetectedGame { exe: "A very long detected game executable with additional edition and platform identifiers.exe".into(), native: true });
    let mut app = App::new(engine);
    let ctx = egui::Context::default();
    theme::install(&ctx, 1.0);
    for size in [egui::vec2(1240.0, 680.0), egui::vec2(760.0, 420.0)] {
        for section in [0usize, 1] {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("profiles-section"), section));
            for pass in 0..3 {
                let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() }, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let available = ui.available_rect_before_wrap();
                        app.page_profiles(ui, &mut Profile::default(), None);
                        if pass == 2 {
                            assert!(ui.min_rect().bottom() <= available.bottom() + 1.0, "profiles content pushes navigation outside {size:?}");
                            assert!(ui.min_rect().right() <= available.right() + 1.0, "game rules exceed {size:?}");
                        }
                    });
                });
            }
        }
    }
    drop(app);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn audio_columns_align_when_one_heading_wraps() {
    let dir = std::env::temp_dir().join(format!("dualwave-audio-layout-{}", std::process::id()));
    let mut app = App::new(Engine::new(dir.clone()));
    let ctx = egui::Context::default();
    theme::install(&ctx, 1.0);
    let output = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 800.0))), ..Default::default() }, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| app.page_audio_workspace(ui, &mut Profile::default(), None));
    });
    let text = |value: &str| output.shapes.iter().find_map(|s| match &s.shape {
        egui::Shape::Text(t) if t.galley.job.text == crate::i18n::tr(value) => Some(t), _ => None,
    }).expect("audio text rendered");
    assert!(!text("Controller speaker / headset").galley.job.justify, "audio heading stretches its word spacing");
    let stream = text("Bluetooth audio stream");
    let microphone = text("Disabled");
    let stream_center = stream.pos.y + stream.galley.size().y * 0.5;
    let microphone_center = microphone.pos.y + microphone.galley.size().y * 0.5;
    assert!((stream_center - microphone_center).abs() < 8.0, "first audio controls must align below headings of different heights");
    drop(app);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn setting_rows_keep_independent_click_targets() {
    let ctx = egui::Context::default();
    theme::install(&ctx, 1.0);
    let mut values = [false; 3];
    let mut target = egui::Pos2::ZERO;
    for frame in 0..4 {
        let events = match frame {
            2 => vec![egui::Event::PointerMoved(target), egui::Event::PointerButton { pos: target, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE }],
            3 => vec![egui::Event::PointerButton { pos: target, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }],
            _ => Vec::new(),
        };
        let _ = ctx.run(egui::RawInput { events, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 300.0))), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for (index, value) in values.iter_mut().enumerate() {
                    widgets::row(ui, "Repeated setting label", "Help for this setting", |ui| {
                        let response = widgets::toggle(ui, value);
                        if index == 1 { target = response.rect.center(); }
                    });
                }
            });
        });
    }
    assert_eq!(values, [false, true, false]);
}

#[test]
fn microphone_control_keeps_capture_and_mute_independent() {
    for (initial_enabled, initial_muted) in [(false, false), (false, true), (true, false), (true, true)] {
        for power in [false, true] {
            let ctx = egui::Context::default();
            theme::install(&ctx, 1.0);
            let (mut enabled, mut muted) = (initial_enabled, initial_muted);
            let mut target = egui::Pos2::ZERO;
            for frame in 0..4 {
                let events = match frame {
                    2 => vec![egui::Event::PointerMoved(target), egui::Event::PointerButton { pos: target, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE }],
                    3 => vec![egui::Event::PointerButton { pos: target, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }],
                    _ => Vec::new(),
                };
                let _ = ctx.run(egui::RawInput { events, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(240.0, 120.0))), ..Default::default() }, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let bounds = ui.available_rect_before_wrap();
                        let (main, toggle) = widgets::microphone_control(ui, &mut enabled, &mut muted, None);
                        target = if power { toggle.rect.center() } else { main.rect.center() };
                        assert!(main.rect.right() <= toggle.rect.left(), "microphone click targets overlap");
                        assert!(toggle.rect.right() <= bounds.right() + 1.0);
                    });
                });
            }
            let expected = if power { (!initial_enabled, initial_muted) }
                else if initial_enabled { (true, !initial_muted) } else { (true, initial_muted) };
            assert_eq!((enabled, muted), expected, "power={power}, initial=({initial_enabled},{initial_muted})");
        }
    }
}

#[test]
fn controller_editors_are_visible_without_an_advanced_menu() {
    let dir = std::env::temp_dir().join(format!("dualwave-navigation-layout-{}", std::process::id()));
    let mut app = App::new(Engine::new(dir.clone()));
    assert_eq!((app.page, app.detailed), (Page::Overview, false), "startup must show the visual controller workspace");
    let ctx = egui::Context::default();
    theme::install(&ctx, 1.0);
    let output = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(760.0, 520.0))), ..Default::default() }, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| app.section_navigation(ui));
    });
    for label in ["Lighting", "Triggers", "Motion", "Buttons", "Virtual controller"] {
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(t) if t.galley.job.text == crate::i18n::tr(label))), "{label} must be directly visible in controller navigation");
    }
    assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(t) if t.galley.job.text == crate::i18n::tr("Advanced"))), "configuration must not be hidden in an Advanced menu");
    drop(app);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn main_navigation_returns_to_the_visual_workspaces() {
    fn render(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(egui::RawInput { events, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(760.0, 520.0))), ..Default::default() }, |ctx| {
            egui::TopBottomPanel::top("navigation").show(ctx, |ui| app.workspace_navigation(ui));
            egui::CentralPanel::default().show(ctx, |ui| app.section_navigation(ui));
        })
    }
    fn click(app: &mut App, ctx: &egui::Context, label: &str) {
        let output = render(app, ctx, Vec::new());
        let target = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(t) if t.galley.job.text == crate::i18n::tr(label) => Some(t.pos + t.galley.rect.center().to_vec2()), _ => None,
        }).expect("navigation destination is directly visible");
        for pressed in [true, false] {
            let _ = render(app, ctx, vec![egui::Event::PointerMoved(target), egui::Event::PointerButton { pos: target, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }]);
        }
    }
    let dir = std::env::temp_dir().join(format!("dualwave-navigation-memory-{}", std::process::id()));
    let mut app = App::new(Engine::new(dir.clone()));
    let ctx = egui::Context::default();
    theme::install(&ctx, 1.0);
    click(&mut app, &ctx, "Motion");
    assert_eq!((app.page, app.detailed), (Page::Input, true));
    click(&mut app, &ctx, "Audio");
    assert_eq!((app.page, app.detailed), (Page::Haptics, false));
    click(&mut app, &ctx, "Haptics & audio");
    assert_eq!((app.page, app.detailed), (Page::Haptics, true));
    click(&mut app, &ctx, "Controller");
    assert_eq!((app.page, app.detailed), (Page::Overview, false));
    click(&mut app, &ctx, "Motion");
    click(&mut app, &ctx, "Workspace");
    assert_eq!((app.page, app.detailed), (Page::Overview, false));
    click(&mut app, &ctx, "Audio");
    assert_eq!((app.page, app.detailed), (Page::Haptics, false));
    drop(app);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn detected_game_can_be_associated_without_enabling_automatic_switching() {
    fn render(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(egui::RawInput { events, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024., 800.))), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.page_profiles(ui, &mut Profile::default(), None));
        })
    }
    fn click(app: &mut App, ctx: &egui::Context, label: &str) {
        let out = render(app, ctx, Vec::new());
        let pos = out.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text == crate::i18n::tr(label) => Some(t.pos + t.galley.rect.center().to_vec2()), _ => None,
        }).expect("association control visible");
        for pressed in [true, false] { render(app, ctx, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }]); }
    }
    let dir = std::env::temp_dir().join(format!("dualwave-game-association-ui-{}", std::process::id()));
    let engine = Engine::new(dir.clone());
    engine.settings.write().auto_profiles = false;
    engine.settings.write().auto_dualsense = false;
    engine.game_status.write().games.push(crate::games::DetectedGame { exe: "example-game.exe".into(), native: false });
    let mut app = App::new(engine.clone());
    let ctx = egui::Context::default(); theme::install(&ctx, 1.0);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("profiles-section"), 1usize));
    click(&mut app, &ctx, "Associate a profile…");
    click(&mut app, &ctx, "Xbox");
    let settings = engine.settings.read();
    assert!(!settings.auto_profiles);
    assert_eq!(settings.games.len(), 1);
    assert_eq!(settings.games[0].exe, "example-game.exe");
    assert_eq!(settings.games[0].profile, "Xbox");
    assert!(engine.game_override.read().is_none());
    assert!(engine.take_game_discovery_request(), "opening game profiles requests discovery even with switching off");
    drop(settings); drop(app); drop(engine);
    let _ = std::fs::remove_dir_all(dir);
}
