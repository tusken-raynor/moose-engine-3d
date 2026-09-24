use std::fs;

/* RENDER CONSTANTS */

pub const DRAW_MODE_AUTO: i8 = -1;
pub const DRAW_MODE_WIREFRAME: i8 = 0;
pub const DRAW_MODE_SOLID: i8 = 1;
pub const DRAW_MODE_TEXTURED: i8 = 2;

pub const LIGHT_MODE_AUTO: i8 = -1;
pub const LIGHT_MODE_FULLY_LIT: i8 = 0;
pub const LIGHT_MODE_PER_PIXEL: i8 = 1;
pub const LIGHT_MODE_PER_REDUCED: i8 = 2;
pub const LIGHT_MODE_PER_VERTEX: i8 = 3;
pub const LIGHT_MODE_PER_FACE: i8 = 4;
pub const LIGHT_MODE_PER_UNIT: i8 = 5;

pub const TEXTURE_MAP_MODE_AUTO: i8 = -1;
pub const TEXTURE_MAP_MODE_PERSPECTIVE: i8 = 0;
pub const TEXTURE_MAP_MODE_REDUCED: i8 = 1;
pub const TEXTURE_MAP_MODE_AFFINE: i8 = 2;
pub const TEXTURE_MAP_MODE_SCREEN: i8 = 3;

pub const TEXTURE_TILE_MODE_REPEAT: i8 = 0;
pub const TEXTURE_TILE_MODE_CLAMP: i8 = 1;
pub const TEXTURE_TILE_MODE_REFLECT: i8 = 2;

pub const TEXTURE_FILTER_MODE_AUTO: i8 = -1;
pub const TEXTURE_FILTER_MODE_NEAREST: u8 = 0;
pub const TEXTURE_FILTER_MODE_NEAREST_MIPMAP_NEAREST: u8 = 1;
pub const TEXTURE_FILTER_MODE_NEAREST_MIPMAP_LINEAR: u8 = 2;
pub const TEXTURE_FILTER_MODE_DITHERED: u8 = 3;
pub const TEXTURE_FILTER_MODE_DITHERED_MIPMAP_NEAREST: u8 = 4;
pub const TEXTURE_FILTER_MODE_DITHERED_MIPMAP_LINEAR: u8 = 5;
pub const TEXTURE_FILTER_MODE_BILINEAR: u8 = 6;
pub const TEXTURE_FILTER_MODE_BILINEAR_MIPMAP_NEAREST: u8 = 7;
pub const TEXTURE_FILTER_MODE_BILINEAR_MIPMAP_LINEAR: u8 = 8;

/* HEAP REGISTRATION */

static mut IS_REGISTERED: bool = false;
static mut REGISTERED_VARS: Option<&'static mut Vars> = None;
static mut REGISTERED_STATE: Option<&'static mut State> = None;

pub struct Vars {
    // Render variables
    pub width: u32,
    pub height: u32,
    pub fov: f32,
    pub znear: f32,
    pub zfar: f32,
    pub frametime: u64,
    pub movespeed: f32,
    pub turnspeed: f32,
    pub maxpitch: f32,
    pub envrate: f32,
    pub userrate: f32,
    pub occlusionquerying: bool,
    pub defaultpalette: String,
    pub drawmode: i8,
    // Software Render variables
    pub coloredlighting: bool,
    pub entlightmode: i8,
    pub seclightmode: i8,
    pub lightmodereduction: u8,
    pub texmapmode: i8,
    pub texmapmodereduction: u8,
    pub texfiltermode: i8,
    // Debug variables
    pub show_occlusions: bool,
    pub show_depth_buffer: bool,
}

impl Vars {
    pub fn get<T: AsRef<str>>(&self, name: T) -> String {
        match name.as_ref() {
            "WIDTH" => self.width.to_string(),
            "HEIGHT" => self.height.to_string(),
            "FOV" => self.fov.to_string(),
            "ZNEAR" => self.znear.to_string(),
            "ZFAR" => self.zfar.to_string(),
            "FPS" => (1000000 / self.frametime).to_string(),
            "MOVESPEED" => self.movespeed.to_string(),
            "TURNSPEED" => self.turnspeed.to_string(),
            "MAXPITCH" => self.maxpitch.to_string(),
            "ENVRATE" => self.envrate.to_string(),
            "USERRATE" => self.userrate.to_string(),
            "OCCLUSIONQUERYING" => self.occlusionquerying.to_string(),
            "DEFAULTPALETTE" => self.defaultpalette.to_string(),
            "DRAWMODE" => self.drawmode.to_string(),
            "COLOREDLIGHTING" => self.coloredlighting.to_string(),
            "ENTLIGHTMODE" => self.entlightmode.to_string(),
            "SECLIGHTMODE" => self.seclightmode.to_string(),
            "LIGHTMODEREDUCTION" => self.lightmodereduction.to_string(),
            "TEXMAPMODE" => self.texmapmode.to_string(),
            "TEXMAPMODEREDUCTION" => self.texmapmodereduction.to_string(),
            "TEXFILTERMODE" => self.texfiltermode.to_string(),
            "SHOWOCCLUSIONS" => self.show_occlusions.to_string(),
            "DEPTHBUFFER" => self.show_depth_buffer.to_string(),
            _ => '0'.to_string(),
        }
    }
    pub fn set<T: AsRef<str>>(&mut self, name: T, value: T) {
        match name.as_ref() {
            "WIDTH" => self.width = value.as_ref().parse::<u32>().unwrap_or(self.width),
            "HEIGHT" => self.height = value.as_ref().parse::<u32>().unwrap_or(self.height),
            "FOV" => self.fov = value.as_ref().parse::<f32>().unwrap_or(self.fov),
            "ZNEAR" => self.znear = value.as_ref().parse::<f32>().unwrap_or(self.znear),
            "ZFAR" => self.zfar = value.as_ref().parse::<f32>().unwrap_or(self.zfar),
            "FPS" => {
                self.frametime = 1000000
                    / value
                        .as_ref()
                        .parse::<u64>()
                        .unwrap_or(1000000 / self.frametime)
            }
            "MOVESPEED" => self.movespeed = value.as_ref().parse::<f32>().unwrap_or(self.movespeed),
            "TURNSPEED" => self.turnspeed = value.as_ref().parse::<f32>().unwrap_or(self.turnspeed),
            "MAXPITCH" => self.maxpitch = value.as_ref().parse::<f32>().unwrap_or(self.maxpitch),
            "ENVRATE" => self.envrate = value.as_ref().parse::<f32>().unwrap_or(self.envrate),
            "USERRATE" => self.userrate = value.as_ref().parse::<f32>().unwrap_or(self.userrate),
            "OCCLUSIONQUERYING" => {
                self.occlusionquerying = value
                    .as_ref()
                    .parse::<bool>()
                    .unwrap_or(self.occlusionquerying)
            }
            "DEFAULTPALETTE" => {
                self.defaultpalette = value
                    .as_ref()
                    .parse::<String>()
                    .unwrap_or(self.defaultpalette.clone())
            }
            "DRAWMODE" => self.drawmode = value.as_ref().parse::<i8>().unwrap_or(self.drawmode),
            "COLOREDLIGHTING" => {
                self.coloredlighting = value
                    .as_ref()
                    .parse::<bool>()
                    .unwrap_or(self.coloredlighting)
            }
            "ENTLIGHTMODE" => {
                self.entlightmode = value.as_ref().parse::<i8>().unwrap_or(self.entlightmode)
            }
            "SECLIGHTMODE" => {
                self.seclightmode = value.as_ref().parse::<i8>().unwrap_or(self.seclightmode)
            }
            "LIGHTMODEREDUCTION" => {
                self.lightmodereduction = value
                    .as_ref()
                    .parse::<u8>()
                    .unwrap_or(self.lightmodereduction)
            }
            "TEXMAPMODE" => {
                self.texmapmode = value.as_ref().parse::<i8>().unwrap_or(self.texmapmode)
            }
            "TEXMAPMODEREDUCTION" => {
                self.texmapmodereduction = value
                    .as_ref()
                    .parse::<u8>()
                    .unwrap_or(self.texmapmodereduction)
            }
            "TEXFILTERMODE" => {
                self.texfiltermode = value.as_ref().parse::<i8>().unwrap_or(self.texfiltermode)
            }
            "SHOWOCCLUSIONS" => {
                self.show_occlusions = value
                    .as_ref()
                    .parse::<bool>()
                    .unwrap_or(self.show_occlusions)
            }
            "DEPTHBUFFER" => {
                self.show_depth_buffer = value
                    .as_ref()
                    .parse::<bool>()
                    .unwrap_or(self.show_depth_buffer)
            }
            _ => (),
        }
    }
    pub fn has<T: AsRef<str>>(&self, name: T) -> bool {
        match name.as_ref() {
            "WIDTH" => true,
            "HEIGHT" => true,
            "FOV" => true,
            "ZNEAR" => true,
            "ZFAR" => true,
            "FPS" => true,
            "MOVESPEED" => true,
            "TURNSPEED" => true,
            "MAXPITCH" => true,
            "ENVRATE" => true,
            "USERRATE" => true,
            "OCCLUSIONQUERYING" => true,
            "DEFAULTPALETTE" => true,
            "DRAWMODE" => true,
            "COLOREDLIGHTING" => true,
            "ENTLIGHTMODE" => true,
            "SECLIGHTMODE" => true,
            "LIGHTMODEREDUCTION" => true,
            "TEXMAPMODE" => true,
            "TEXMAPMODEREDUCTION" => true,
            "TEXFILTERMODE" => true,
            "SHOWOCCLUSIONS" => true,
            "DEPTHBUFFER" => true,
            _ => false,
        }
    }
}

/* DYNAMIC VARIABLES */

// Create a hashmap that can hold multiple variable types

/* PLAYER STATE */

pub struct State {
    pub dude_x: f32,
    pub dude_y: f32,
    pub dude_z: f32,
    pub dude_yaw: f32,
    pub dude_pitch: f32,
    pub moving_forward: bool,
    pub moving_backward: bool,
    pub moving_left: bool,
    pub moving_right: bool,
    pub pitching_up: bool,
    pub pitching_down: bool,
    pub yawing_left: bool,
    pub yawing_right: bool,
    pub flying: bool,
}

pub fn register_config(vars: &'static mut Vars, state: &'static mut State) {
    // Register the config variables by placing references
    // to the struct on the heap. This way we can access
    // them from any module
    unsafe {
        if IS_REGISTERED {
            return;
        }
        IS_REGISTERED = true;

        REGISTERED_VARS = Some(vars);
        REGISTERED_STATE = Some(state);
    }
}

pub fn load_config(filepath: String) -> (Vars, f32, State) {
    // Load the config json file and parse it
    let cfg_json = fs::read_to_string(filepath).ok();
    let cfg = json::parse(cfg_json.unwrap_or(String::from("{}")).as_str()).unwrap();

    let has_vars = cfg.has_key("Vars") && cfg["Vars"].is_object();

    let res = if has_vars && cfg["Vars"].has_key("RES") {
        cfg["Vars"]["RES"].as_f32().unwrap()
    } else {
        1.0
    };

    /* ENGINE VARIABLES */
    let vars = Vars {
        width: if has_vars && cfg["Vars"].has_key("WIDTH") {
            (cfg["Vars"]["WIDTH"].as_f32().unwrap() * res) as u32
        } else {
            (640.0 * res) as u32
        },
        height: if has_vars && cfg["Vars"].has_key("HEIGHT") {
            (cfg["Vars"]["HEIGHT"].as_f32().unwrap() * res) as u32
        } else {
            (480.0 * res) as u32
        },
        fov: if has_vars && cfg["Vars"].has_key("FOV") {
            cfg["Vars"]["FOV"].as_f32().unwrap()
        } else {
            90.0
        },
        znear: if has_vars && cfg["Vars"].has_key("ZNEAR") {
            cfg["Vars"]["ZNEAR"].as_f32().unwrap()
        } else {
            0.1
        },
        zfar: if has_vars && cfg["Vars"].has_key("ZFAR") {
            cfg["Vars"]["ZFAR"].as_f32().unwrap()
        } else {
            1000.0
        },
        frametime: if has_vars && cfg["Vars"].has_key("FPS") {
            1000000 / cfg["Vars"]["FPS"].as_u64().unwrap()
        } else {
            16666
        },
        movespeed: if has_vars && cfg["Vars"].has_key("MOVESPEED") {
            cfg["Vars"]["MOVESPEED"].as_f32().unwrap()
        } else {
            1.0
        },
        turnspeed: if has_vars && cfg["Vars"].has_key("TURNSPEED") {
            cfg["Vars"]["TURNSPEED"].as_f32().unwrap()
        } else {
            2.0
        },
        maxpitch: if has_vars && cfg["Vars"].has_key("MAXPITCH") {
            cfg["Vars"]["MAXPITCH"].as_f32().unwrap()
        } else {
            80.0
        },
        userrate: if has_vars && cfg["Vars"].has_key("USERRATE") {
            cfg["Vars"]["USERRATE"].as_f32().unwrap()
        } else {
            1.0
        },
        envrate: if has_vars && cfg["Vars"].has_key("ENVRATE") {
            cfg["Vars"]["ENVRATE"].as_f32().unwrap()
        } else {
            1.0
        },
        occlusionquerying: if has_vars && cfg["Vars"].has_key("OCCLUSIONQUERYING") {
            cfg["Vars"]["OCCLUSIONQUERYING"].as_bool().unwrap()
        } else {
            true
        },
        defaultpalette: if has_vars && cfg["Vars"].has_key("DEFAULTPALETTE") {
            cfg["Vars"]["DEFAULTPALETTE"].as_str().unwrap().to_string()
        } else {
            String::from("dflt.cmp")
        },
        drawmode: if has_vars && cfg["Vars"].has_key("DRAWMODE") {
            if cfg["Vars"]["DRAWMODE"].is_number() {
                cfg["Vars"]["DRAWMODE"].as_i8().unwrap()
            } else {
                match cfg["Vars"]["DRAWMODE"].as_str().unwrap() {
                    "DRAW_MODE_AUTO" => DRAW_MODE_AUTO,
                    "DRAW_MODE_WIREFRAME" => DRAW_MODE_WIREFRAME,
                    "DRAW_MODE_SOLID" => DRAW_MODE_SOLID,
                    "DRAW_MODE_TEXTURED" => DRAW_MODE_TEXTURED,
                    _ => DRAW_MODE_AUTO,
                }
            }
        } else {
            DRAW_MODE_AUTO
        },
        coloredlighting: if has_vars && cfg["Vars"].has_key("COLOREDLIGHTING") {
            cfg["Vars"]["COLOREDLIGHTING"].as_bool().unwrap()
        } else {
            true
        },
        entlightmode: if has_vars && cfg["Vars"].has_key("ENTLIGHTMODE") {
            if cfg["Vars"]["ENTLIGHTMODE"].is_number() {
                cfg["Vars"]["ENTLIGHTMODE"].as_i8().unwrap()
            } else {
                match cfg["Vars"]["ENTLIGHTMODE"].as_str().unwrap() {
                    "LIGHT_MODE_PER_PIXEL" => LIGHT_MODE_PER_PIXEL,
                    "LIGHT_MODE_PER_VERTEX" => LIGHT_MODE_PER_VERTEX,
                    "LIGHT_MODE_PER_FACE" => LIGHT_MODE_PER_FACE,
                    "LIGHT_MODE_PER_UNIT" => LIGHT_MODE_PER_UNIT,
                    _ => LIGHT_MODE_PER_VERTEX,
                }
            }
        } else {
            LIGHT_MODE_PER_VERTEX
        },
        seclightmode: if has_vars && cfg["Vars"].has_key("SECLIGHTMODE") {
            if cfg["Vars"]["SECLIGHTMODE"].is_number() {
                cfg["Vars"]["SECLIGHTMODE"].as_i8().unwrap()
            } else {
                match cfg["Vars"]["SECLIGHTMODE"].as_str().unwrap() {
                    "LIGHT_MODE_PER_PIXEL" => LIGHT_MODE_PER_PIXEL,
                    "LIGHT_MODE_PER_VERTEX" => LIGHT_MODE_PER_VERTEX,
                    "LIGHT_MODE_PER_FACE" => LIGHT_MODE_PER_FACE,
                    "LIGHT_MODE_PER_UNIT" => LIGHT_MODE_PER_UNIT,
                    "LIGHT_MODE_FULLY_LIT" => LIGHT_MODE_FULLY_LIT,
                    "LIGHT_MODE_PER_REDUCED" => LIGHT_MODE_PER_REDUCED,
                    _ => LIGHT_MODE_PER_REDUCED,
                }
            }
        } else {
            LIGHT_MODE_PER_PIXEL
        },
        lightmodereduction: if has_vars && cfg["Vars"].has_key("LIGHTMODEREDUCTION") {
            cfg["Vars"]["LIGHTMODEREDUCTION"].as_u8().unwrap()
        } else {
            16
        },
        texmapmode: if has_vars && cfg["Vars"].has_key("TEXMAPMODE") {
            if cfg["Vars"]["TEXMAPMODE"].is_number() {
                cfg["Vars"]["TEXMAPMODE"].as_i8().unwrap()
            } else {
                match cfg["Vars"]["TEXMAPMODE"].as_str().unwrap() {
                    "TEXTURE_MAP_MODE_AFFINE" => TEXTURE_MAP_MODE_AFFINE,
                    "TEXTURE_MAP_MODE_PERSPECTIVE" => TEXTURE_MAP_MODE_PERSPECTIVE,
                    "TEXTURE_MAP_MODE_REDUCED" => TEXTURE_MAP_MODE_REDUCED,
                    "TEXTURE_MAP_MODE_SCREEN" => TEXTURE_MAP_MODE_SCREEN,
                    "TEXTURE_MAP_MODE_AUTO" => TEXTURE_MAP_MODE_AUTO,
                    _ => TEXTURE_MAP_MODE_AUTO,
                }
            }
        } else {
            TEXTURE_MAP_MODE_PERSPECTIVE
        },
        texmapmodereduction: if has_vars && cfg["Vars"].has_key("TEXMAPMODEREDUCTION") {
            cfg["Vars"]["TEXMAPMODEREDUCTION"].as_u8().unwrap()
        } else {
            16
        },
        texfiltermode: if has_vars && cfg["Vars"].has_key("TEXFILTERMODE") {
            if cfg["Vars"]["TEXFILTERMODE"].is_number() {
                cfg["Vars"]["TEXFILTERMODE"].as_i8().unwrap()
            } else {
                match cfg["Vars"]["TEXFILTERMODE"].as_str().unwrap() {
                    "TEXTURE_FILTER_MODE_NEAREST" => TEXTURE_FILTER_MODE_NEAREST as i8,
                    "TEXTURE_FILTER_MODE_NEAREST_MIPMAP_NEAREST" => {
                        TEXTURE_FILTER_MODE_NEAREST_MIPMAP_NEAREST as i8
                    }
                    "TEXTURE_FILTER_MODE_NEAREST_MIPMAP_LINEAR" => {
                        TEXTURE_FILTER_MODE_NEAREST_MIPMAP_LINEAR as i8
                    }
                    "TEXTURE_FILTER_MODE_DITHERED" => TEXTURE_FILTER_MODE_DITHERED as i8,
                    "TEXTURE_FILTER_MODE_DITHERED_MIPMAP_NEAREST" => {
                        TEXTURE_FILTER_MODE_DITHERED_MIPMAP_NEAREST as i8
                    }
                    "TEXTURE_FILTER_MODE_DITHERED_MIPMAP_LINEAR" => {
                        TEXTURE_FILTER_MODE_DITHERED_MIPMAP_LINEAR as i8
                    }
                    "TEXTURE_FILTER_MODE_BILINEAR" => TEXTURE_FILTER_MODE_BILINEAR as i8,
                    "TEXTURE_FILTER_MODE_BILINEAR_MIPMAP_NEAREST" => {
                        TEXTURE_FILTER_MODE_BILINEAR_MIPMAP_NEAREST as i8
                    }
                    "TEXTURE_FILTER_MODE_BILINEAR_MIPMAP_LINEAR" => {
                        TEXTURE_FILTER_MODE_BILINEAR_MIPMAP_LINEAR as i8
                    }
                    _ => TEXTURE_FILTER_MODE_AUTO,
                }
            }
        } else {
            TEXTURE_FILTER_MODE_AUTO
        },
        show_occlusions: if has_vars && cfg["Vars"].has_key("SHOWOCCLUSIONS") {
            cfg["Vars"]["SHOWOCCLUSIONS"].as_bool().unwrap()
        } else {
            false
        },
        show_depth_buffer: if has_vars && cfg["Vars"].has_key("DEPTHBUFFER") {
            cfg["Vars"]["DEPTHBUFFER"].as_bool().unwrap()
        } else {
            false
        },
    };
    /* ENGINE VARIABLES */

    /* STATE */
    let state = State {
        dude_x: 0.0,
        dude_y: -0.0,
        dude_z: -0.0,
        dude_yaw: 0.0,
        dude_pitch: 0.0,
        moving_forward: false,
        moving_backward: false,
        moving_left: false,
        moving_right: false,
        pitching_up: false,
        pitching_down: false,
        yawing_left: false,
        yawing_right: false,
        flying: true,
    };
    /* STATE */

    // Register the config
    // register_config(&vars, &state);

    // Return the variables and state to be used by the main process
    return (vars, res, state);
}
