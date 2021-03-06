#[macro_use]
extern crate gfx;
extern crate ggez;

use ggez::*;
use ggez::event::*;
use ggez::graphics::*;
use std::env;
use std::path;

gfx_defines!{
    /// Constants used by the shaders to calculate stuff
    constant Light {
        light_color: [f32; 4] = "u_Color",
        shadow_color: [f32; 4] = "u_ShadowColor",
        pos: [f32; 2] = "u_Pos",
        screen_size: [f32; 2] = "u_ScreenSize",
        glow: f32 = "u_Glow",
        strength: f32 = "u_Strength",
    }
}

/// Shader source for calculating a 1D shadow map that encodes half distances
/// in the red channel. The idea is that we scan X rays (X is the horizontal
/// size of the output) and calculate the distance to the nearest pixel at that
/// angle that has transparency above a threashold. The distance gets halved
/// and encoded in the red channel (it is havled because if the distance can be
/// greater than 1.0 - think bottom left to top right corner, that sqrt(1) and
/// will not get properly encoded).
const OCCLUSIONS_SHADER_SOURCE: &[u8] = b"#version 150 core

uniform sampler2D t_Texture;
in vec2 v_Uv;
out vec4 Target0;

layout (std140) uniform Light {
    vec4 u_Color;
    vec4 u_ShadowColor;
    vec2 u_Pos;
    vec2 u_ScreenSize;
    float u_Glow;
    float u_Strength;
};

void main() {
    float dist = 1.0;
    float theta = radians(v_Uv.x * 360.0);
    vec2 dir = vec2(cos(theta), sin(theta));
    for(int i = 0; i < 1024; i++) {
        float fi = i;
        float r = fi / 1024.0;
        vec2 rel = r * dir;
        vec2 p = clamp(u_Pos+rel, 0.0, 1.0);
        if (texture(t_Texture, p).a > 0.8) {
            dist = distance(u_Pos, p) * 0.5;
            break;
        }
    }

    float others = dist == 1.0 ? 0.0 : dist;
    Target0 = vec4(dist, others, others, 1.0);
}
";

/// Shader for drawing shadows based on a 1D shadow map. It takes current
/// fragment cordinates and converts them to polar coordinates centered
/// around the light source, using the angle to sample from the 1D shadow map.
/// If the distance from the light source is greater than the distance of the
/// closest reported shadow, then the output is black, else it calculates some
/// shadow based on the distance from light source based on strength and glow
/// uniform parameters.
const SHADOWS_SHADER_SOURCE: &[u8] = b"#version 150 core

uniform sampler2D t_Texture;
in vec2 v_Uv;
out vec4 Target0;

layout (std140) uniform Light {
    vec4 u_Color;
    vec4 u_ShadowColor;
    vec2 u_Pos;
    vec2 u_ScreenSize;
    float u_Glow;
    float u_Strength;
};

void main() {
    vec2 coord = gl_FragCoord.xy / u_ScreenSize;
    vec2 rel = coord - u_Pos;
    float theta = atan(rel.y, rel.x);
    float ox = degrees(theta) / 360.0;
    if (ox < 0) {
        ox += 1.0;
    }
    float r = length(rel);
    float occl = texture(t_Texture, vec2(ox, 0.5)).r * 2.0;

    float intensity = 0.9;
    if (r < occl) {
        vec2 g = u_ScreenSize / u_ScreenSize.y;
        float p = u_Strength + u_Glow;
        float d = distance(g * coord, g * u_Pos);
        intensity = 0.9 - clamp(p/(d*d), 0.0, 0.9);
    }

    Target0 = vec4(u_ShadowColor.rgb / 80.0, intensity);
}
";

struct MainState {
    background: Image,
    tile: Image,
    text: Text,
    light: Light,
    foreground: Canvas,
    occlusions: Canvas,
    occlusions_shader: PixelShader<Light>,
    shadows_shader: PixelShader<Light>,
}

/// The color cast things take when not illuminated
const AMBIENT_COLOR: [f32; 4] = [0.30, 0.30, 0.53, 1.0];
/// The default color for the light
const LIGHT_COLOR: [f32; 4] = [0.72, 0.64, 0.32, 1.0];
/// The number of rays to cast to. Increasing this number will result in better
/// quality shadows. If you increase too much you might hit some GPU shader
/// hardware limits.
const LIGHT_RAY_COUNT: u32 = 1440;
/// The strength of the light - how far it shines
const LIGHT_STRENGTH: f32 = 0.01;
/// The factor at which the light glows - just for fun
const LIGHT_GLOW_FACTOR: f32 = 0.001;
/// The rate at which the glow effect oscillates
const LIGHT_GLOW_RATE: f32 = 50.0;

impl MainState {
    fn new(ctx: &mut Context) -> GameResult<MainState> {
        let background = Image::new(ctx, "/bg_top.png")?;
        let tile = Image::new(ctx, "/tile.png")?;
        let text = {
            let font = Font::new(ctx, "/DejaVuSerif.ttf", 48)?;
            Text::new(ctx, "SHADOWS...", &font)?
        };
        let screen_size = {
            let size = ctx.gfx_context.get_drawable_size();
            [size.0 as f32, size.1 as f32]
        };
        let light = Light {
            pos: [0.0, 0.0],
            light_color: LIGHT_COLOR,
            shadow_color: AMBIENT_COLOR,
            screen_size,
            glow: 0.0,
            strength: LIGHT_STRENGTH,
        };
        let foreground = Canvas::with_window_size(ctx)?;
        let occlusions = Canvas::new(ctx, LIGHT_RAY_COUNT, 1, conf::NumSamples::One)?;
        let occlusions_shader =
            PixelShader::from_u8(ctx, OCCLUSIONS_SHADER_SOURCE, light, "Light")?;
        let shadows_shader = PixelShader::from_u8(ctx, SHADOWS_SHADER_SOURCE, light, "Light")?;

        Ok(MainState {
            background,
            tile,
            text,
            light,
            foreground,
            occlusions,
            occlusions_shader,
            shadows_shader,
        })
    }
}

impl event::EventHandler for MainState {
    fn update(&mut self, ctx: &mut Context) -> GameResult<()> {
        if timer::get_ticks(ctx) % 100 == 0 {
            println!("Average FPS: {}", timer::get_fps(ctx));
        }

        self.light.glow = LIGHT_GLOW_FACTOR *
            ((timer::get_ticks(ctx) as f32) / LIGHT_GLOW_RATE).cos();
        Ok(())
    }

    fn draw(&mut self, ctx: &mut Context) -> GameResult<()> {
        let size = ctx.gfx_context.get_size();
        let center = DrawParam {
            dest: Point2::new(size.0 as f32 / 2.0, size.1 as f32 / 2.0),
            ..Default::default()
        };
        // for re-rendering canvases, we need to take the DPI into account
        let dpiscale = {
            let dsize = ctx.gfx_context.get_drawable_size();
            Point2::new(
                size.0 as f32 / dsize.0 as f32,
                size.1 as f32 / dsize.1 as f32,
            )
        };
        let canvascenter = DrawParam {
            scale: dpiscale,
            ..center
        };

        // First thing we want to do it to render all the foreground items (that
        // will have shadows) onto their own Canvas (off-scree render). We will
        // use this canvas to:
        //  - run the occlusions shader to determine where the shadows are
        //  - render to screen once all the shadows are caculated and rendered
        graphics::set_canvas(ctx, Some(&self.foreground));
        graphics::set_background_color(ctx, [0.0; 4].into());
        graphics::clear(ctx);
        graphics::draw_ex(
            ctx,
            &self.tile,
            DrawParam {
                dest: Point2::new(598.0, 124.0),
                ..Default::default()
            },
        )?;
        graphics::draw_ex(
            ctx,
            &self.tile,
            DrawParam {
                dest: Point2::new(92.0, 350.0),
                ..Default::default()
            },
        )?;
        graphics::draw_ex(
            ctx,
            &self.tile,
            DrawParam {
                dest: Point2::new(442.0, 468.0),
                rotation: 0.5,
                ..Default::default()
            },
        )?;
        graphics::draw_ex(ctx, &self.text, center)?;

        // Now we want to run the occlusions shader to calculate our 1D shadow
        // distances into the `occlusions` canvas.
        {
            let _shader_lock = graphics::use_shader(ctx, &self.occlusions_shader);
            self.occlusions_shader.send(ctx, self.light)?;

            graphics::set_canvas(ctx, Some(&self.occlusions));
            graphics::draw_ex(ctx, &self.foreground, canvascenter)?;
        }

        // Now lets finally render to screen starting with out background, then
        // the shadows overtop and finally our foreground. Note that we set the
        // light color as the color for our render giving everything the "tint"
        // we desire.
        graphics::set_canvas(ctx, None);
        // color filter so things take the light color
        graphics::set_color(ctx, self.light.light_color.into())?;
        graphics::clear(ctx);
        graphics::draw_ex(ctx, &self.background, center)?;
        {
            let _shader_lock = graphics::use_shader(ctx, &self.shadows_shader);
            self.shadows_shader.send(ctx, self.light)?;

            let param = DrawParam {
                scale: Point2::new((size.0 as f32) / (LIGHT_RAY_COUNT as f32), (size.1 as f32)),
                ..center
            };
            graphics::draw_ex(ctx, &self.occlusions, param)?;
        }
        graphics::set_color(ctx, AMBIENT_COLOR.into())?;
        graphics::draw_ex(ctx, &self.foreground, canvascenter)?;

        // Uncomment following two lines to visualize the 1D occlusions canvas,
        // red pixels represent angles at which no shadows were found, and then
        // the greyscale pixels are the half distances of the nearest shadows to
        // the mouse position (equally encoded in all color channels).
        // graphics::set_color(ctx, [1.0; 4].into())?;
        // graphics::draw_ex(ctx, &self.occlusions, center)?;

        graphics::present(ctx);
        Ok(())
    }

    fn mouse_motion_event(
        &mut self,
        ctx: &mut Context,
        _state: MouseState,
        x: i32,
        y: i32,
        _xrel: i32,
        _yrel: i32,
    ) {
        let (w, h) = ctx.gfx_context.get_size();
        let (x, y) = (x as f32 / w as f32, 1.0 - y as f32 / h as f32);
        self.light.pos = [x, y];
    }
}

pub fn main() {
    let c = conf::Conf::new();
    let ctx = &mut Context::load_from_conf("shadows", "ggez", c).unwrap();

    // We add the CARGO_MANIFEST_DIR/resources do the filesystems paths so
    // we we look in the cargo project for files.
    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let mut path = path::PathBuf::from(manifest_dir);
        path.push("resources");
        ctx.filesystem.mount(&path, true);
    }

    let state = &mut MainState::new(ctx).unwrap();
    event::run(ctx, state).unwrap();
}
