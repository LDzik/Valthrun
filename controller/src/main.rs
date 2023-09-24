#![feature(iterator_try_collect)]
#![feature(result_option_inspect)]
#![allow(dead_code)]

use anyhow::Context;
use cache::EntryCache;
use clap::{Parser, Args, Subcommand};
use cs2::{CS2Handle, Module, CS2Offsets, EntitySystem, CS2Model, Globals, BuildInfo};
use cs2_schema_generated::{definition::SchemaScope, RuntimeOffsetProvider, RuntimeOffset};
use enhancements::Enhancement;
use imgui::{Condition, Ui};
use obfstr::obfstr;
use settings::{load_app_settings, AppSettings};
use settings_ui::SettingsUI;
use std::{
    cell::{RefCell, RefMut},
    fmt::Debug,
    fs::File,
    io::BufWriter,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use view::ViewController;
use windows::Win32::System::Console::GetConsoleProcessList;

use crate::{
    enhancements::{AntiAimPunsh, BombInfo, PlayerESP, TriggerBot},
    settings::save_app_settings,
    view::LocalCrosshair, winver::version_info,
};

mod view;
mod settings;
mod settings_ui;
mod cache;
mod enhancements;
mod winver;

pub trait UpdateInputState {
    fn is_key_down(&self, key: imgui::Key) -> bool;
    fn is_key_pressed(&self, key: imgui::Key, repeating: bool) -> bool;
}

impl UpdateInputState for imgui::Ui {
    fn is_key_down(&self, key: imgui::Key) -> bool {
        Ui::is_key_down(self, key)
    }

    fn is_key_pressed(&self, key: imgui::Key, repeating: bool) -> bool {
        if repeating {
            Ui::is_key_pressed(self, key)
        } else {
            Ui::is_key_pressed_no_repeat(self, key)
        }
    }
}

pub struct UpdateContext<'a> {
    pub settings: &'a AppSettings,
    pub input: &'a dyn UpdateInputState,

    pub cs2: &'a Arc<CS2Handle>,
    pub cs2_entities: &'a EntitySystem,

    pub model_cache: &'a EntryCache<u64, CS2Model>,
    pub class_name_cache: &'a EntryCache<u64, Option<String>>,
    pub view_controller: &'a ViewController,

    pub globals: Globals,
}

pub struct Application {
    pub cs2: Arc<CS2Handle>,
    pub cs2_offsets: Arc<CS2Offsets>,
    pub cs2_entities: EntitySystem,
    pub cs2_globals: Option<Globals>,
    pub cs2_build_info: BuildInfo,

    pub model_cache: EntryCache<u64, CS2Model>,
    pub class_name_cache: EntryCache<u64, Option<String>>,
    pub view_controller: ViewController,

    pub enhancements: Vec<Rc<RefCell<dyn Enhancement>>>,

    pub frame_read_calls: usize,
    pub last_total_read_calls: usize,

    pub settings: Rc<RefCell<AppSettings>>,
    pub settings_visible: bool,
    pub settings_dirty: bool,
    pub settings_ui: RefCell<SettingsUI>,
}

impl Application {
    pub fn settings(&self) -> std::cell::Ref<'_, AppSettings> {
        self.settings.borrow()
    }

    pub fn settings_mut(&self) -> RefMut<'_, AppSettings> {
        self.settings.borrow_mut()
    }

    pub fn pre_update(&mut self, context: &mut imgui::Context) -> anyhow::Result<()> {
        if self.settings_dirty {
            self.settings_dirty = false;
            let mut settings = self.settings.borrow_mut();

            let mut imgui_settings = String::new();
            context.save_ini_settings(&mut imgui_settings);
            settings.imgui = Some(imgui_settings);

            if let Err(error) = save_app_settings(&*settings) {
                log::warn!("Failed to save user settings: {}", error);
            };
        }
        Ok(())
    }

    pub fn update(&mut self, ui: &imgui::Ui) -> anyhow::Result<()> {
        {
            let mut settings = self.settings.borrow_mut();
            for enhancement in self.enhancements.iter() {
                let mut hack = enhancement.borrow_mut();
                if hack.update_settings(ui, &mut *settings)? {
                    self.settings_dirty = true;
                }
            }
        }

        let settings = self.settings.borrow();
        if ui.is_key_pressed_no_repeat(settings.key_settings.0) {
            log::debug!("Toogle settings");
            self.settings_visible = !self.settings_visible;

            if !self.settings_visible {
                /* overlay has just been closed */
                self.settings_dirty = true;
            }
        }

        self.view_controller
            .update_screen_bounds(mint::Vector2::from_slice(&ui.io().display_size));
        self.view_controller.update_view_matrix(&self.cs2)?;

        let globals = self
            .cs2
            .reference_schema::<Globals>(&[self.cs2_offsets.globals, 0])?
            .cached()
            .with_context(|| obfstr!("failed to read globals").to_string())?;

        let update_context = UpdateContext {
            cs2: &self.cs2,
            cs2_entities: &self.cs2_entities,

            settings: &*settings,
            input: ui,

            globals,
            class_name_cache: &self.class_name_cache,
            view_controller: &self.view_controller,
            model_cache: &self.model_cache,
        };

        for enhancement in self.enhancements.iter() {
            let mut hack = enhancement.borrow_mut();
            hack.update(&update_context)?;
        }

        let read_calls = self.cs2.ke_interface.total_read_calls();
        self.frame_read_calls = read_calls - self.last_total_read_calls;
        self.last_total_read_calls = read_calls;

        Ok(())
    }

    pub fn render(&self, ui: &imgui::Ui) {
        ui.window("overlay")
            .draw_background(false)
            .no_decoration()
            .no_inputs()
            .size(ui.io().display_size, Condition::Always)
            .position([0.0, 0.0], Condition::Always)
            .build(|| self.render_overlay(ui));

        if self.settings_visible {
            let mut settings_ui = self.settings_ui.borrow_mut();
            settings_ui.render(self, ui)
        }
    }

    fn render_overlay(&self, ui: &imgui::Ui) {
        let settings = self.settings.borrow();

        {
            let text_buf;
            let text = obfstr!(text_buf = "Valthrun Overlay");

            ui.set_cursor_pos([
                ui.window_size()[0] - ui.calc_text_size(text)[0] - 10.0,
                10.0,
            ]);
            ui.text(text);
        }
        {
            let text = format!("{:.2} FPS", ui.io().framerate);
            ui.set_cursor_pos([
                ui.window_size()[0] - ui.calc_text_size(&text)[0] - 10.0,
                24.0,
            ]);
            ui.text(text)
        }
        {
            let text = format!("{} Reads", self.frame_read_calls);
            ui.set_cursor_pos([
                ui.window_size()[0] - ui.calc_text_size(&text)[0] - 10.0,
                38.0,
            ]);
            ui.text(text)
        }

        for hack in self.enhancements.iter() {
            let hack = hack.borrow();
            hack.render(&*settings, ui, &self.view_controller);
        }
    }
}

fn show_critical_error(message: &str) {
    log::error!("{}", message);

    if !is_console_invoked() {
        overlay::show_error_message(obfstr!("Valthrun Controller"), message);
    }
}

fn main() {
    let args = match AppArgs::try_parse() {
        Ok(args) => args,
        Err(error) => {
            println!("{:#}", error);
            std::process::exit(1);
        }
    };

    env_logger::builder()
        .filter_level(if args.verbose {
            log::LevelFilter::Trace
        } else {
            log::LevelFilter::Info
        })
        .parse_default_env()
        .init();

    let command = args.command.as_ref().unwrap_or(&AppCommand::Overlay);
    let result = match command {
        AppCommand::DumpSchema(args) => main_schema_dump(args),
        AppCommand::Overlay => main_overlay(),
    };

    if let Err(error) = result {
        show_critical_error(&format!("{:#}", error));
    }
}

#[derive(Debug, Parser)]
#[clap(name = "Valthrun", version)]
struct AppArgs {
    /// Enable verbose logging ($env:RUST_LOG="trace")
    #[clap(short, long)]
    verbose: bool,

    #[clap(subcommand)]
    command: Option<AppCommand>,
}

#[derive(Debug, Subcommand)]
enum AppCommand {
    /// Start the overlay
    Overlay,

    /// Create a schema dump
    DumpSchema(SchemaDumpArgs),
}

#[derive(Debug, Args)]
struct SchemaDumpArgs {
    pub target_file: PathBuf,
}

fn is_console_invoked() -> bool {
    let console_count = unsafe {
        let mut result = [0u32; 128];
        GetConsoleProcessList(&mut result)
    };

    console_count > 1
}

fn main_schema_dump(args: &SchemaDumpArgs) -> anyhow::Result<()> {
    log::info!("Dumping schema. Please wait...");

    let cs2 = CS2Handle::create()?;
    let schema = cs2::dump_schema(&cs2, false)?;

    let output = File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&args.target_file)?;

    let mut output = BufWriter::new(output);
    serde_json::to_writer_pretty(&mut output, &schema)?;
    log::info!("Schema dumped to {}", args.target_file.to_string_lossy());
    Ok(())
}

struct CS2RuntimeOffsets {
    schema: Vec<SchemaScope>,
}

impl RuntimeOffsetProvider for CS2RuntimeOffsets {
    fn resolve(&self, offset: &RuntimeOffset) -> anyhow::Result<u64> {
        log::trace!("Try resolve {:?}", offset);

        let schema = self.schema.iter()
            .find(|schema| schema.schema_name == offset.module)
            .context("unknown module")?;

        let class = schema.classes.iter()
            .find(|class| offset.class == class.class_name)
            .context("unknown class")?;

        let offset = class.offsets.iter()
            .find(|member| member.field_name == offset.member)
            .context("unknown class member")?;

        log::trace!(" -> {:X}", offset.offset);
        Ok(offset.offset)
    }
}

fn setup_runtime_offset_provider(cs2: &Arc<CS2Handle>) -> anyhow::Result<()> {
    let schema = cs2::dump_schema(&cs2, true)?;
    cs2_schema_generated::setup_runtime_offset_provider(
        Box::new(CS2RuntimeOffsets {
            schema
        })
    );
    Ok(())
}

fn main_overlay() -> anyhow::Result<()> {
    let build_info = version_info()?;
    log::info!("Valthrun v{}. Windows build {}.", env!("CARGO_PKG_VERSION"), build_info.dwBuildNumber);
    
    let settings = load_app_settings()?;

    let cs2 = CS2Handle::create()?;
    let cs2_build_info = BuildInfo::read_build_info(&cs2).with_context(|| {
        obfstr!("Failed to load CS2 build info. CS2 version might be newer / older then expected")
            .to_string()
    })?;
    log::info!(
        "Found {}. Revision {} from {}.",
        obfstr!("Counter-Strike 2"),
        cs2_build_info.revision,
        cs2_build_info.build_datetime
    );

    let cs2_offsets = Arc::new(
        CS2Offsets::resolve_offsets(&cs2)
            .with_context(|| obfstr!("failed to load CS2 offsets").to_string())?,
    );

    setup_runtime_offset_provider(&cs2)?;

    let imgui_settings = settings.imgui.clone();
    let settings = Rc::new(RefCell::new(settings));
    let app = Application {
        cs2: cs2.clone(),
        cs2_entities: EntitySystem::new(cs2.clone(), cs2_offsets.clone()),
        cs2_offsets: cs2_offsets.clone(),
        cs2_globals: None,
        cs2_build_info,

        model_cache: EntryCache::new({
            let cs2 = cs2.clone();
            move |model| {
                let model_name = cs2.read_string(&[*model as u64 + 0x08, 0], Some(32))?;
                log::debug!(
                    "{} {}. Caching.",
                    obfstr!("Discovered new player model"),
                    model_name
                );

                Ok(CS2Model::read(&cs2, *model as u64)?)
            }
        }),
        class_name_cache: EntryCache::new({
            let cs2 = cs2.clone();
            move |vtable: &u64| {
                let fn_get_class_schema = cs2.reference_schema::<u64>(&[
                    *vtable + 0x00, // First entry in V-Table is GetClassSchema
                ])?;

                let mut asm_buffer = [0u8; 0x10];
                cs2.read_slice(&[fn_get_class_schema], &mut asm_buffer)?;

                // lea rcx, <class schema>
                if asm_buffer[9] != 0x48 || asm_buffer[10] != 0x8D || asm_buffer[11] != 0x15 {
                    /* Class defined in other module. GetClassSchema function might be implemented diffrently. */
                    log::trace!("{:X} isn't a client schema class. Returning none.", vtable);
                    return Ok(None);
                }

                let schema_offset = i32::from_le_bytes(asm_buffer[12..16].try_into()?) as u64;
                let class_schema = fn_get_class_schema
                    .wrapping_add(schema_offset)
                    .wrapping_add(0x10);

                if !cs2.module_address(Module::Client, class_schema).is_some() {
                    log::warn!(
                        "GetClassSchema lea points to invalid target address for {:X}",
                        *vtable
                    );
                    return Ok(None);
                }

                let class_name = cs2.read_string(&[class_schema + 0x08, 0], Some(32))?;
                log::trace!("Resolved vtable class name {:X} to {}", vtable, class_name);
                Ok(Some(class_name))
            }
        }),
        view_controller: ViewController::new(cs2_offsets.clone()),

        enhancements: vec![
            Rc::new(RefCell::new(PlayerESP::new())),
            Rc::new(RefCell::new(BombInfo::new())),
            Rc::new(RefCell::new(TriggerBot::new(LocalCrosshair::new(
                cs2_offsets.offset_crosshair_id,
            )))),
            Rc::new(RefCell::new(AntiAimPunsh::new())),
        ],

        last_total_read_calls: 0,
        frame_read_calls: 0,

        settings: settings.clone(),
        settings_visible: false,
        settings_dirty: false,
        settings_ui: RefCell::new(SettingsUI::new(settings)),
    };

    let app = Rc::new(RefCell::new(app));

    log::debug!("Initialize overlay");
    let mut overlay = overlay::init(obfstr!("CS2 Overlay"), obfstr!("Counter-Strike 2"))?;
    if let Some(imgui_settings) = imgui_settings {
        overlay.imgui.load_ini_settings(&imgui_settings);
    }

    log::info!("{}", obfstr!("App initialized. Spawning overlay."));
    let mut update_fail_count = 0;
    let mut update_timeout: Option<(Instant, Duration)> = None;
    overlay.main_loop(
        {
            let app = app.clone();
            move |context| {
                let mut app = app.borrow_mut();
                if let Err(err) = app.pre_update(context) {
                    show_critical_error(&format!("{:#}", err));
                    false
                } else {
                    true
                }
            }
        },
        move |ui| {
            let mut app = app.borrow_mut();

            if let Some((timeout, target)) = &update_timeout {
                if timeout.elapsed() > *target {
                    update_timeout = None;
                } else {
                    /* Not updating. On timeout... */
                    return true;
                }
            }

            if let Err(err) = app.update(ui) {
                if update_fail_count >= 10 {
                    log::error!("Over 10 errors occurred. Waiting 1s and try again.");
                    log::error!("Last error: {:#}", err);

                    update_timeout = Some((Instant::now(), Duration::from_millis(1000)));
                    update_fail_count = 0;
                    return true;
                } else {
                    update_fail_count += 1;
                }
            }

            app.render(ui);
            true
        },
    )
}
