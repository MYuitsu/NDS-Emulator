use super::{GPU, Color, geometry::Vertex, Engine3D, super::VRAM, TextureFormat};

impl Engine3D {
    pub fn get_line(&self, vcount: u16, line: &mut [u16; GPU::WIDTH]) {
        for (i, pixel) in line.iter_mut().enumerate() {
            *pixel = self.pixels[vcount as usize * GPU::WIDTH + i]
        }
    }

    pub fn render(&mut self, vram: &VRAM) {
        if !self.polygons_submitted { return }
        // TODO: Add more accurate interpolation
        // TODO: Optimize
        // TODO: Textures
        for (i, pixel) in self.pixels.iter_mut().enumerate() {
            *pixel = self.clear_color.color();
            self.depth_buffer[i] = self.clear_depth.depth();
        }

        assert!(!self.frame_params.w_buffer); // TODO: Implement W-Buffer
        // TODO: Account for special cases
        // TODO: Remove with const generics
        fn eq_depth_test(cur_depth: u32, new_depth: u32) -> bool {
            new_depth >= cur_depth - 0x200 && new_depth <= cur_depth + 0x200
        }
        fn lt_depth_test(cur_depth: u32, new_depth: u32) -> bool { new_depth < cur_depth }

        
        for polygon in self.polygons.iter() {
            // TODO: Remove with const generics
            let blend = |color, s, t| {
                let vram_offset = polygon.tex_params.vram_offset;
                let pal_offset = polygon.palette_base;
                let texel = t * polygon.tex_params.size_s + s;
                match polygon.tex_params.format {
                    TextureFormat::NoTexture => color,
                    TextureFormat::Palette4 => {
                        let palette_color = vram.get_textures::<u8>(vram_offset + texel / 4) >> 2 * (texel % 4) & 0x3;
                        vram.get_textures_pal::<u16>(pal_offset / 2 + 2 * palette_color as usize)
                    }
                    TextureFormat::Palette16 => {
                        let palette_color = vram.get_textures::<u8>(vram_offset + texel / 2) >> 4 * (texel % 2) & 0xF;
                        vram.get_textures_pal::<u16>(pal_offset + 2 * palette_color as usize)
                    },
                    TextureFormat::DirectColor => vram.get_textures::<u16>(vram_offset + 2 * texel),
                }
            };
            // TODO: Use fixed point for interpolation
            // TODO: Fix uneven interpolation
            let depth_test = if polygon.attrs.depth_test_eq { eq_depth_test } else { lt_depth_test };
            let vertices = &self.vertices[polygon.start_vert..polygon.end_vert];
            if polygon.attrs.render_front {
                Engine3D::render_polygon(blend, &mut self.depth_buffer, depth_test, true, &mut self.pixels, vertices)
            }
            if polygon.attrs.render_back {
                Engine3D::render_polygon(blend, &mut self.depth_buffer, depth_test, false, &mut self.pixels, vertices)
            }
        }

        self.gxstat.geometry_engine_busy = false;
        self.vertices.clear();
        self.polygons.clear();
        self.polygons_submitted = false;
    }

    // TODO: Replace front with a const generic
    fn render_polygon<B, D>(blend: B, depth_buffer: &mut Vec<u32>, depth_test: D, front: bool, pixels: &mut Vec<u16>, vertices: &[Vertex])
    where B: Fn(u16, usize, usize) -> u16, D: Fn(u32, u32) -> bool {
        assert!(vertices.len() >= 3);

        // Check if front or back side is being rendered
        let a = (
            vertices[0].screen_coords[0] as i32 - vertices[1].screen_coords[0] as i32,
            vertices[0].screen_coords[1] as i32 - vertices[1].screen_coords[1] as i32,
        );
        let b = (
            vertices[2].screen_coords[0] as i32 - vertices[1].screen_coords[0] as i32,
            vertices[2].screen_coords[1] as i32 - vertices[1].screen_coords[1] as i32,
        );
        let cross_product = a.1 * b.0 - a.0 * b.1;
        let can_draw = match cross_product {
            0 => { info!("Not Drawing Line"); false }, // Line - TODO
            _ if cross_product < 0 => front, // Front
            _ if cross_product > 0 => !front, // Back
            _ => unreachable!(),
        };
        if !can_draw { return }
        
        // Find top left and bottom right vertices
        let (mut start_vert, mut end_vert) = (0, 0);
        for (i, vert) in vertices.iter().enumerate() {
            if vert.screen_coords[1] < vertices[start_vert].screen_coords[1] {
                start_vert = i;
            } else if vert.screen_coords[1] == vertices[start_vert].screen_coords[1] &&
                vert.screen_coords[0] < vertices[start_vert].screen_coords[0] {
                start_vert = i;
            }

            if vert.screen_coords[1] > vertices[end_vert].screen_coords[1] {
                end_vert = i;
            } else if vert.screen_coords[1] == vertices[end_vert].screen_coords[1] &&
                vert.screen_coords[0] > vertices[end_vert].screen_coords[0] {
                end_vert = i;
            }
        }
        let mut left_vert = start_vert;
        let mut right_vert = start_vert;
        let start_vert = start_vert; // Shadow to mark these as immutable
        let end_vert = end_vert; // Shadow to mark these as immutable

        let counterclockwise = |cur| if cur == vertices.len() - 1 { 0 } else { cur + 1 };
        let clockwise = |cur| if cur == 0 { vertices.len() - 1 } else { cur - 1 };

        let (prev_vert, next_vert): (Box<dyn Fn(usize) -> usize>, Box<dyn Fn(usize) -> usize>) = if front {
            (Box::new(counterclockwise), Box::new(clockwise))
        } else {
            (Box::new(clockwise), Box::new(counterclockwise))
        };
        let new_left_vert = prev_vert(left_vert);
        let mut left_slope = VertexSlope::from_verts(
            &vertices[left_vert],
            &vertices[new_left_vert],
        );
        let mut left_end = vertices[new_left_vert].screen_coords[1];
        left_vert = new_left_vert;
        let new_right_vert = next_vert(right_vert);
        let mut right_slope = VertexSlope::from_verts(
            &vertices[right_vert],
            &vertices[new_right_vert],
        );
        let mut right_end = vertices[new_right_vert].screen_coords[1];
        right_vert = new_right_vert;

        for y in vertices[start_vert].screen_coords[1]..vertices[end_vert].screen_coords[1] {
            // While loops to skip repeated vertices from clipping
            // TODO: Should this be fixed in clipping or rendering code?
            while y == left_end {
                let new_left_vert = prev_vert(left_vert);
                left_slope = VertexSlope::from_verts(&vertices[left_vert], &vertices[new_left_vert]);
                left_end = vertices[new_left_vert].screen_coords[1];
                left_vert = new_left_vert;
            }
            while y == right_end {
                let new_right_vert = next_vert(right_vert);
                right_slope = VertexSlope::from_verts(&vertices[right_vert],&vertices[new_right_vert]);
                right_end = vertices[new_right_vert].screen_coords[1];
                right_vert = new_right_vert;
            }
            let x_start = left_slope.next_x() as usize;
            let x_end = right_slope.next_x() as usize;
            let num_steps = x_end - x_start;
            let mut color = ColorSlope::new(
                &left_slope.next_color(),
                &right_slope.next_color(),
                num_steps,
            );
            let mut s = Slope::new(
                left_slope.next_s(),
                right_slope.next_s(),
                num_steps,
            );
            let mut t = Slope::new(
                left_slope.next_t(),
                right_slope.next_t(),
                num_steps,
            );
            let mut depth = Slope::new(
                left_slope.next_depth(),
                right_slope.next_depth(),
                num_steps,
            );

            for x in x_start..x_end {
                let depth_val = depth.next() as u32;
                if depth_test(depth_buffer[y * GPU::WIDTH + x], depth_val) {
                    depth_buffer[y * GPU::WIDTH + x] = depth_val;
                    let blended_color = blend(color.next().as_u16(), s.next() as usize >> 4, t.next() as usize >> 4);
                    pixels[y * GPU::WIDTH + x] = 0x8000 | blended_color;
                }
            }
        }
    }
}

#[derive(Debug)]
struct Slope {
    cur: f32,
    step: f32,
}

impl Slope {
    pub fn new(start: f32, end: f32, num_steps: usize) -> Self {
        Slope {
            cur: start,
            step: (end - start) / num_steps as f32,
        }
    }

    pub fn next(&mut self) -> f32 {
        let return_val = self.cur;
        self.cur += self.step;
        return_val
    }
}

struct VertexSlope {
    x: Slope,
    s: Slope,
    t: Slope,
    depth: Slope,
    color: ColorSlope,
}

impl VertexSlope {
    pub fn from_verts(start: &Vertex, end: &Vertex) -> Self {
        let num_steps = end.screen_coords[1] - start.screen_coords[1];
        // TODO: Implement w-buffer
        VertexSlope {
            x: Slope::new(start.screen_coords[0] as f32, end.screen_coords[0] as f32, num_steps),
            s: Slope::new(start.tex_coord[0] as f32, end.tex_coord[0] as f32, num_steps),
            t: Slope::new(start.tex_coord[1] as f32, end.tex_coord[1] as f32, num_steps),
            depth: Slope::new(start.z_depth as f32, end.z_depth as f32, num_steps),
            color: ColorSlope::new(
                &start.color,
                &end.color,
                end.screen_coords[1] - start.screen_coords[1],
            ),
        }
    }

    pub fn next_x(&mut self) -> f32 {
        self.x.next()
    }

    pub fn next_s(&mut self) -> f32 {
        self.s.next()
    }

    pub fn next_t(&mut self) -> f32 {
        self.t.next()
    }

    pub fn next_depth(&mut self) -> f32 {
        self.depth.next()
    }

    pub fn next_color(&mut self) -> Color {
        self.color.next()
    }
}

struct ColorSlope {
    r: Slope,
    g: Slope,
    b: Slope,
}

impl ColorSlope {
    pub fn new(start_color: &Color, end_color: &Color, num_steps: usize) -> Self {
        ColorSlope {
            r: Slope::new(start_color.r as f32, end_color.r as f32, num_steps),
            g: Slope::new(start_color.g as f32, end_color.g as f32, num_steps),
            b: Slope::new(start_color.b as f32, end_color.b as f32, num_steps),
        }
    }

    pub fn next(&mut self) -> Color {
        Color::new(
            self.r.next() as u8,
            self.g.next() as u8,
            self.b.next() as u8,
        )
    }
}
