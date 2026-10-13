// A lift, for an entity or a template (script=lift.ts): used (E), it rises, and used
// again, it comes back down. Its lamp, if it is given one, shows which way it went.
import { GO, STOP, fade } from "./util/lights";

const rise = input(1.0);
const seconds = input(2.0);
const lamp = input<Light | null>();

export let up = false;
let busy = false;

export async function onActivated(target: Entity, by: Player) {
  if (busy) return;
  busy = true;
  up = !up;
  // Both at once, and on when both are done.
  await Promise.all([
    self.moveBy(vec3(0, up ? rise : -rise, 0), seconds),
    fade(lamp, up ? GO : STOP, seconds / 2),
  ]);
  busy = false;
}
