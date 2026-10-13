// Colors and light helpers that scripts share. A file like this one, which others import,
// holds only functions and constants.

export const GO = rgb(40, 255, 40);
export const STOP = rgb(255, 40, 40);

/** Fades `light` to `to` over `seconds`, if there is a light. */
export async function fade(light: Light | null, to: Color, seconds: number) {
  if (light !== null) await light.fadeTo(to, seconds);
}
