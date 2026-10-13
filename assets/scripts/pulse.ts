// Makes a light breathe: it dims by `depth` of its color and comes back, once every
// `period` seconds.

const light = input<Light>();
const period = input(4.0);
const depth = input(0.5);

const full = light.color;

export function onTick(dt: number) {
  const swell = 0.5 + 0.5 * Math.cos((time.now / period) * 2 * Math.PI);
  light.color = full.scale(1 - depth * (1 - swell));
}
