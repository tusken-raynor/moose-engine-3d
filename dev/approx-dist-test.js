const maxDist = 255;

function approx_dist(deltaX, deltaY, deltaZ) {
  let absX = Math.abs(deltaX);
  let absY = Math.abs(deltaY);
  let absZ = Math.abs(deltaZ);
  let absC = approx_dist_2d(absX, absY);
  let max = Math.max(absC, absZ);
  let min = Math.min(absC, absZ);
  let approx = 1007 * max + 441 * min;
  if (max < min * 16) {
    approx -= max * 40;
  }
  return approx / 1024;
}

function approx_dist_2d(deltaX, deltaY) {
  let absX = Math.abs(deltaX);
  let absY = Math.abs(deltaY);
  let max = Math.max(absX, absY);
  let min = Math.min(absX, absY);
  let approx = 1007 * max + 441 * min;
  if (max < min * 16) {
    approx -= max * 40;
  }
  return approx / 1024;
}

function approx_dist_3d(dx, dy, dz) {
  let max_xy = Math.max(Math.abs(dx), Math.abs(dy));
  let max = Math.max(max_xy, Math.abs(dz));
  let min_xy = Math.min(Math.abs(dx), Math.abs(dy));
  let min = Math.min(min_xy, Math.abs(dz));
  let approx =
    ((max << 10) -
      (max << 4) -
      max +
      (min << 9) -
      (min << 6) -
      (min << 3) +
      min) >>
    10;
  return approx;
}

function true_dist(deltaX, deltaY, deltaZ) {
  return Math.sqrt(deltaX * deltaX + deltaY * deltaY + deltaZ * deltaZ);
}

let max_error = 0;
let error_perf = 0;
let error_ratio = 0;
let min_error = 1000000;
let min_error_perf = 0;
let min_error_ratio = 0;
for (let x = 0; x <= maxDist; x++) {
  for (let y = 0; y <= maxDist; y++) {
    for (let z = 0; z <= maxDist; z++) {
      let approx = approx_dist(x, y, z);
      let true_val = true_dist(x, y, z);
      let error = Math.abs(approx - true_val);
      if (error > max_error) {
        max_error = error;
        error_perf = (error / true_val) * 100;
        error_ratio = approx / true_val;
      }
      if (error < min_error) {
        min_error = error;
      }
    }
  }
}

console.log(`Maximum error: ${max_error} (${error_perf}%) [${error_ratio}]`);
console.log(`Minimum error: ${min_error}`);

function testApproximateDist() {
  const xEnd = 256;
  const yEnd = 256;
  const zEnd = 256;

  let maxError = 0;
  let maxErrorPcnt = 0;
  let avgError = 0;
  let avgErrorPcnt = 0;
  let rIdx = 0;
  for (let x = 0; x < xEnd; x++) {
    for (let y = 0; y < yEnd; y++) {
      for (let z = 0; z < zEnd; z++) {
        // const w = approximateDist(x, y);
        const dist = approximateDist3D(x, y, z);
        const actualDist = Math.sqrt(x * x + y * y + z * z);
        const error = Math.abs(dist - actualDist);
        if (error > maxError) {
          maxError = error;
          maxErrorPcnt = (error / actualDist) * 100;
        }
        avgError += error;
        avgErrorPcnt += actualDist ? (error / actualDist) * 100 : 0;
        if (Math.random() < 16 / 16777216) {
          rIdx++;
          console.log(
            `${rIdx}: x: ${x}, y: ${y}, z: ${z}, dist: ${dist}, actualDist: ${actualDist}`
          );
        }
      }
    }
  }
  avgError /= xEnd * yEnd * zEnd;
  avgErrorPcnt /= xEnd * yEnd * zEnd;
  console.log(`Max error: ${maxError}`);
  console.log(`Max error %: ${maxErrorPcnt}`);
  console.log(`Avg error: ${avgError}`);
  console.log(`Avg error %: ${avgErrorPcnt}`);
}

function approximateDist(x, y) {
  const min = Math.min(x, y);
  const max = Math.max(x, y);
  let approx = 1007 * max + 441 * min;
  if (max < min * 16) {
    approx -= max * 40;
  }
  return approx / 1024;
}
function approximateDist3D(x, y, z) {
  const min = Math.min(x, y, z);
  const max = Math.max(x, y, z);
  let approx = 1007 * max + 441 * min;
  if (max < min * 16) {
    approx -= max * 40;
  }
  return approx / 1024;
}

testApproximateDist();