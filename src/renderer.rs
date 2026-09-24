use crate::{camera::Camera, entity::Entity, threedee::{pythag, dot_product}, rasterizer::{qfetch_pixel, fill_pixel_safe, set_texture_sample_function, fill_polygon_2, fill_polygon_1, draw_polygon}, utils::sort_things, matrix::Transformation, resources::{self, get_archetype}, setup::{Vars, TEXTURE_MAP_MODE_AFFINE, DRAW_MODE_WIREFRAME, DRAW_MODE_SOLID}, fragment::get_fragment_function_index};

const MIPMAP_DISTANCES: [f32; 4] = [1.0, 2.0, 3.0, 4.0];

pub fn render_sector_statics() {

}

pub fn render_sector_entities(
  camera: &Camera,
  mut depth_buff: &mut Vec<u32>, 
  mut frame_buff: &mut Vec<u32>,  
  view_transform: &Transformation,
  view_rotation: &Transformation,
  // Pass a clone of the entities vector so we can sort it
  mut entities: Vec<Entity>,
  vars: &Vars
) {
  // The buffer width
  let width = vars.width as usize;

  // OPT Once we are using portal rendering, we only need to sort the things by sector
  sort_things(&mut entities, &|a, b| {
    pythag(view_transform.transform_point(a.get_pos()))
      > pythag(view_transform.transform_point(b.get_pos()))
  });

  for entity in &entities {
      // OPT You can combine the view and object matrices into one matrix right here
      // Pull the archetype from the list using the index from entity
      let archetype = resources::get_archetype(entity.archetype);
      // Pull the model from the list using the index from archetype
      let model = resources::get_model(archetype.model);

      // if the model has more than one mesh, check each indiviual mesh for visibility
      if model.meshes.len() > 1 {
        // Determine if the 3d bounds of the model lie within the field of view
        // OPT You can combine the view and object matrices into one matrix right here
        // OPT You can treat the six numbers as two points and only transform those two points
        let rect_transformed = [
          view_transform.transform_point(entity.transform_point([
            model.rect[0],
            model.rect[1],
            model.rect[2],
          ])),
          view_transform.transform_point(entity.transform_point([
            model.rect[0],
            model.rect[1],
            model.rect[5],
          ])),
          view_transform.transform_point(entity.transform_point([
            model.rect[0],
            model.rect[4],
            model.rect[2],
          ])),
          view_transform.transform_point(entity.transform_point([
            model.rect[0],
            model.rect[4],
            model.rect[5],
          ])),
          view_transform.transform_point(entity.transform_point([
            model.rect[3],
            model.rect[1],
            model.rect[2],
          ])),
          view_transform.transform_point(entity.transform_point([
            model.rect[3],
            model.rect[1],
            model.rect[5],
          ])),
          view_transform.transform_point(entity.transform_point([
            model.rect[3],
            model.rect[4],
            model.rect[2],
          ])),
          view_transform.transform_point(entity.transform_point([
            model.rect[3],
            model.rect[4],
            model.rect[5],
          ])),
        ];
        // Pass this 3d rect to the function. If the rect is in view, it will set the
        // values of the 3d rect indicating where on the screen to do the occlusion query
        let visible = run_rect_visibility_query(
          &camera, 
          &mut depth_buff, 
          &mut frame_buff, 
          vars.width as usize, 
          vars.height as usize, 
          rect_transformed, 
          vars.occlusionquerying
            && vars.drawmode != DRAW_MODE_WIREFRAME
            && entity.is_occludee(), 
          vars.show_occlusions,
        );
        if !visible {
          continue;
        }
      }
      
      for mesh in &model.meshes {
          // Determine if the 3d bounds of the mesh lie within the field of view
          // OPT You can combine the view and object matrices into one matrix right here
          // OPT You can treat the six numbers as two points and only transform those two points
          let rect_transformed = [
              view_transform.transform_point(entity.transform_point([
                  mesh.rect[0],
                  mesh.rect[1],
                  mesh.rect[2],
              ])),
              view_transform.transform_point(entity.transform_point([
                  mesh.rect[0],
                  mesh.rect[1],
                  mesh.rect[5],
              ])),
              view_transform.transform_point(entity.transform_point([
                  mesh.rect[0],
                  mesh.rect[4],
                  mesh.rect[2],
              ])),
              view_transform.transform_point(entity.transform_point([
                  mesh.rect[0],
                  mesh.rect[4],
                  mesh.rect[5],
              ])),
              view_transform.transform_point(entity.transform_point([
                  mesh.rect[3],
                  mesh.rect[1],
                  mesh.rect[2],
              ])),
              view_transform.transform_point(entity.transform_point([
                  mesh.rect[3],
                  mesh.rect[1],
                  mesh.rect[5],
              ])),
              view_transform.transform_point(entity.transform_point([
                  mesh.rect[3],
                  mesh.rect[4],
                  mesh.rect[2],
              ])),
              view_transform.transform_point(entity.transform_point([
                  mesh.rect[3],
                  mesh.rect[4],
                  mesh.rect[5],
              ])),
          ];
          // Pass this 3d rect to the function. If the rect is in view, it will set the
          // values of the 3d rect indicating where on the screen to do the occlusion query
          let visible = run_rect_visibility_query(
            &camera, 
            &mut depth_buff, 
            &mut frame_buff, 
            vars.width as usize, 
            vars.height as usize, 
            rect_transformed, 
            vars.occlusionquerying
              && vars.drawmode != DRAW_MODE_WIREFRAME
              && entity.is_occludee(), 
            vars.show_occlusions,
          );
          // println!("{} is visible: {}", model.name, visible);
          if !visible {
            continue;
          }

          // transform all the verts of the mesh
          let mut view_space_verts: Vec<[f32; 3]> = vec![];
          let mut world_space_verts: Vec<[f32; 3]> = vec![];
          for vert in &mesh.verts {
            let point = entity.transform_point(*vert);
            world_space_verts.push(point);
            view_space_verts.push(view_transform.transform_point(point));
          }
          // transform all the normals of the mesh
          let mut world_space_normals: Vec<[f32; 3]> = vec![];
          for normal in &mesh.normals {
            world_space_normals.push(entity.transform_vector(*normal));
          }


          // Determine the texture map mode for this mesh
          let texture_map_mode = if vars.texmapmode < 0 {
            mesh.texmapmode as i8
          } else {
            vars.texmapmode 
          };

          // Regular static archetype render code
          for face in &mesh.faces {
              // Transform the face normal and determine if it is backfacing using one point from the face
              // OPT You can combine the view and object matrices into one matrix right here
              let normal = view_rotation.transform_point(entity.transform_vector(face.normal));
              let mut backfacing = false;
              // Determine the interval at which persective correction needs to be calculated
              // based on how direct the camera is looking at the face
              let reduce_base: f32 = vars.width as f32 * 0.01;
              let dir_dot = dot_product(normal, [0.0, 0.0, 1.0]);
              let dir_dot = (0.0 - dir_dot).max(0.0);
              let reduction = (dir_dot * reduce_base * 4.0 + reduce_base) as usize;
              // let reduction = vars.texmapmodereduction as usize;
              let vector_dot = dot_product(normal, view_space_verts[face.verts[0]]);
              if !face.dualsided && vector_dot >= 0.0 {
                if vars.drawmode != DRAW_MODE_WIREFRAME {
                  continue;
                } else {
                  backfacing = true;
                }
              }

              let mut points: Vec<[f32; 11]> = vec![];
              let mut i = 0;
              let len = face.verts.len();
              while i < len {
                let vert = view_space_verts[face.verts[i]];
                let world_vert = world_space_verts[face.verts[i]];
                let normal =
                    if face.normals.len() > i && mesh.normals.len() > face.normals[i] {
                        world_space_normals[face.normals[i]]
                    } else {
                        face.normal
                    };
                let uvs = if face.uvs.len() > i && mesh.uvs.len() > face.uvs[i] {
                    mesh.uvs[face.uvs[i]]
                } else {
                    [0.0, 0.0]
                };
                let point = [
                    vert[0],
                    vert[1],
                    vert[2],
                    normal[0],
                    normal[1],
                    normal[2],
                    uvs[0],
                    uvs[1],
                    world_vert[0],
                    world_vert[1],
                    world_vert[2],
                ];
                points.push(point);
                i += 1;
              }
              
              let mut y_bounds = (vars.height as usize, 0);
              let projected_points = camera.project_face(points, &mut y_bounds, &MIPMAP_DISTANCES.to_vec());

              // Only render a "face" if it has at least 3 points
              if projected_points.len() > 2 {
                  if vars.drawmode != DRAW_MODE_WIREFRAME {
                    let material = resources::get_material(face.material as usize);
                    if vars.drawmode != DRAW_MODE_SOLID && face.material != 0 {
                      let filtermode = if vars.texfiltermode < 0 {
                        face.filtermode as i8
                      } else {
                        vars.texfiltermode 
                      };
                      // Grab the index of the function pointer for the texture sampler
                      let index = get_fragment_function_index(
                        &material.channel_format, 
                        filtermode as u8
                      );
                      set_texture_sample_function(index);
                      // if face.material == 3 {
                      //   println!("{} dot {}", dir_dot, reduction);
                      // }
                      fill_polygon_2(
                        &mut frame_buff,
                        &mut depth_buff,
                        width,
                        projected_points,
                        y_bounds,
                        material,
                        texture_map_mode,
                        reduction,
                        true,
                      );
                    } else {
                      let texture = resources::get_texture(material.texture as usize);
                      fill_polygon_1(
                        projected_points,
                        texture.color,
                        vars.texmapmode != TEXTURE_MAP_MODE_AFFINE,
                        true,
                        width,
                        y_bounds,
                        &mut frame_buff,
                        &mut depth_buff,
                      );
                    }
                    // panic!("");
                  } else {
                      // Render the scene as wireframes
                    draw_polygon(
                        &mut frame_buff,
                        projected_points
                            .iter()
                            .map(|x| (x.x as isize, x.y as isize, x.z))
                            .collect(),
                        if backfacing { 0xA50000 } else { 0xFF0000 },
                        vars.width,
                        vars.height,
                    );
                  }
              }
          }
      }
  }
}

pub fn run_rect_visibility_query(
  camera: &Camera, 
  mut depth_buff: &mut Vec<u32>, 
  mut frame_buff: &mut Vec<u32>, 
  buffer_width: usize,
  buffer_height: usize,
  rect: [[f32; 3]; 8], 
  do_occlusion_querying: bool,
  debug: bool,
) -> bool {
  // If occlusion querying is enabled, this rect will be written to
  // with the occlusion query rectangle
  let mut occl_rect: (usize, usize, usize, usize, u32) = (0, 0, 0, 0, 0);
  let in_view = camera.rect_in_view(
      rect,
      do_occlusion_querying,
      &mut occl_rect,
  );
  if !in_view {
      return false;
  }
  if do_occlusion_querying
      && occl_rect.0 != occl_rect.1
      && occl_rect.2 != occl_rect.3
  {
      let mut y = occl_rect.3;
      let mut occluded = true;
      while y <= occl_rect.2 {
          let buffer_row = y * buffer_width;
          let mut x = occl_rect.0 + (y & 1);
          while x <= occl_rect.1 {
              let index = buffer_row + x;
              let depth = qfetch_pixel(&mut depth_buff, index);

              if occl_rect.4 < depth {
                  occluded = false;
                  break;
              }
              x += 2;
          }
          if !occluded {
              break;
          }
          y += 1;
      }
      if occluded {
          if debug {
              let mut y = occl_rect.3;
              while y <= occl_rect.2 {
                  let mut x = occl_rect.0 + (y & 1);
                  while x <= occl_rect.1 {
                      fill_pixel_safe(
                          &mut frame_buff,
                          x as isize,
                          y as isize,
                          255,
                          buffer_width as u32,
                          buffer_height as u32,
                      );
                      fill_pixel_safe(
                          &mut depth_buff,
                          x as isize,
                          y as isize,
                          0,
                          buffer_width as u32,
                          buffer_height as u32,
                      );
                      x += 2;
                  }
                  y += 1;
              }
          }
          return false;
      }
  }
  return true;
}