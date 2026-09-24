use crate::{interpolate::{interpolate_u32_pc, interpolate_f32, interpolate_i32_pc}, vertex::ProjectedVertex, resources};

#[derive(Debug, Clone)]
pub struct Span {
  pub x1: usize,
  pub x2: usize,
  pub w1: f32,
  pub w2: f32,
  pub z1: u32,
  pub z2: u32,
  pub u1: i32,
  pub u2: i32,
  pub v1: i32,
  pub v2: i32,
  pub mat_id: u32,
  pub tex_id: u32,
}

pub fn clip_span(span: &mut Span, new_x: usize, clip_beginning: bool) {
  let alpha = (new_x as f32 - span.x1 as f32) / (span.x2 as f32 - span.x1 as f32);

  // Interpolate z, u, v, and w attributes with perspective correction
  let new_z = interpolate_u32_pc(span.z1, span.z2, span.w1, span.w2, alpha);
  let new_u = interpolate_i32_pc(span.u1, span.u2, span.w1, span.w2, alpha);
  let new_v = interpolate_i32_pc(span.v1, span.v2, span.w1, span.w2, alpha);
  let new_w = interpolate_f32(span.w1, span.w2, alpha);

  if clip_beginning {
      span.x1 = new_x;
      span.z1 = new_z;
      span.u1 = new_u;
      span.v1 = new_v;
      span.w1 = new_w;
  } else {
      span.x2 = new_x;
      span.z2 = new_z;
      span.u2 = new_u;
      span.v2 = new_v;
      span.w2 = new_w;
  }
}

pub fn put_polygon_to_span_buffer(
  span_buffer: &mut Vec<Vec<Span>>,
  points: Vec<ProjectedVertex>,
  y_bounds: (usize, usize),
  material_id: u32,
)  {
  // Grab the material id and the texture id
  let material = resources::get_material(material_id as usize);
  let y_diff = y_bounds.1 - y_bounds.0;
  let mut spans = vec![Span {
      x1: 0,
      x2: 0,
      w1: 0.0,
      w2: 0.0,
      z1: 0,
      z2: 0,
      u1: 0,
      u2: 0,
      v1: 0,
      v2: 0,
      mat_id: material_id,
      tex_id: material.texture,
  }; y_diff];
  // Let's loop through the convex polygon and
  // determine the end points of each span
  let mut i = 0;
  let len = points.len();
  let mut j = len - 1;
  while i < len {
    let p1 = points[j];
    let p2 = points[i];
    // If the line is not horizontal
    if p1.y != p2.y {
      // If the line is not vertical
      fill_span_data_from_line(p1, p2, &mut spans);
    }
    j = i;
    i += 1;
  }
  // We should now have the span data we want
  // Let's blit them to the span buffer
  for (i, span) in spans.iter().enumerate() {
    let y = y_bounds.0 + i;
    if span.x1 >= span.x2 {
      // The x1 must be less than x2
      // OPT: If we set up the code right, we can porbably remove this check
      continue;
    }
    // Here we need to check the span buffer to see if we need to clip the span
    // We grab the row from the span buffer using the y value
    // Then we need to use a binary search to if an existing span overlaps
    // the ends of the new span. If it does, we need to determine which span
    // has a lower z value and clip the ends of the new span to that span
    let mut span_row = &mut span_buffer[y];
    let mut span_idx = 0;
    let mut clip_beginning = false;
    let mut clip_end = false;
    let mut new_x = span.x1;
    let mut new_span = span.clone();
    while span_idx < span_row.len() {
      let existing_span = &span_row[span_idx];
      if existing_span.x1 > span.x2 {
        // We have reached the end of the existing spans
        break;
      }
      if existing_span.x2 < span.x1 {
        // This span is before the new span
        span_idx += 1;
        continue;
      }
      // We have an overlap
      // Let's determine which span has a lower z value
      if existing_span.z1 < span.z1 {
        // The existing span has a lower z value
        // We need to clip the beginning of the new span
        clip_beginning = true;
        new_x = existing_span.x2;
      } else {
        // The new span has a lower z value
        // We need to clip the end of the new span
        clip_end = true;
        new_x = existing_span.x1;
      }
      break;
    }
  }
}

fn fill_span_data_from_line(point1: ProjectedVertex, point2: ProjectedVertex, spans: &mut Vec<Span>) {
  // Set the points in order of which one has the highest y value
  let reorder: bool = point1.y > point2.y;
  let p1 = if reorder { point2 } else { point1 };
  let p2 = if reorder { point1 } else { point2 };
  // Loop through the each y pos of the line and fill in the span data
  let mut y = 0;
  let end = p2.y - p1.y;
  // Determine the step of the linear components: x, alpha, and w
  // We need to shift the X value into fixed point space for interpolation
  let x_step = (((p2.x - p1.x) as i32) << 16) / end as i32;
  let mut x = (p1.x as i32) << 16;
  let w_step = (p2.w - p1.w) / end as f32;
  let mut w = p1.w;
  let alpha_step = 1.0 / end as f32;
  let mut alpha = 0.0;
  while y < end {
      let z = interpolate_u32_pc(p1.z, p2.z, p1.w, p2.w, alpha);
      let u = interpolate_i32_pc(p1.u, p2.u, p1.w, p2.w, alpha);
      let v = interpolate_i32_pc(p1.v, p2.v, p1.w, p2.w, alpha);
      let span = &mut spans[y];
      // Return the X back into regular screen space
      let rsx = (x >> 16) as usize;
      if rsx < span.x1 {
          span.x1 = rsx;
          span.z1 = z;
          span.u1 = u;
          span.v1 = v;
          span.w1 = w;
      } else {
          span.x2 = rsx;
          span.z2 = z;
          span.u2 = u;
          span.v2 = v;
          span.w2 = w;
      }
      y += 1;
      x += x_step;
      w += w_step;
      alpha += alpha_step;
  }
}

fn find_span_clip_points(buffer_row: &Vec<Span>, span: &Span) -> (usize, usize) {
  let mut left_x = span.x1;
  let mut right_x = span.x2;
  let mut mid = buffer_row.len() / 2;
  let mut start = 0;
  let mut end = buffer_row.len() - 1;
  let mut found_left = false;
  // First do a binary search to find where the left needs to be clipped
  while start <= end {
      let existing_span = &buffer_row[mid];
      if span.x1 >= existing_span.x1 && span.x1 <= existing_span.x2 {
        // We found a potential match, do a depth test
        if span.z1 < existing_span.z1 {
          // The new span is in front of the existing span
          left_x = existing_span.x2;
          found_left = true;
        } else {
          // The new span is behind the existing span
          // We need to clip the end of the new span
          right_x = existing_span.x1;
        }
      }
      if existing_span.x1 > span.x2 {
          // The new span is before the existing span
          start = mid + 1;
          mid = (start + end) / 2;
          continue;
      }
      // We have an overlap
      // Let's determine which span has a lower z value
      if existing_span.z1 < span.z1 {
          // The existing span has a lower z value
          // We need to clip the beginning of the new span
          left_x = existing_span.x2;
          found_left = true;
      } else {
          // The new span has a lower z value
          // We need to clip the end of the new span
          right_x = existing_span.x1;
      }
      break;
  }
  return (left_x, right_x);
}


// If span end does not site on any other span, we can early out for that end
// However, if this happens for both ends, make sure to keep track of the last 'mid'
// value for each end. If the last mid isn't the same for both ends, then we know
// there's a span behind it that needs to be checked
