// A ball, for an entity with a mass (mass=5 shape=ball script=ball.ts): it thumps when it
// lands on something, louder the harder it lands, and hops when it is used (E).

const thump = input<Sound | null>();
const hop = input(4.0);

export let thumps = 0;

export async function onTouched(target: Entity, other: Entity | null, speed: number) {
  if (speed < 1 || thump === null) return;
  thumps += 1;
  level.set("thumps", thumps);
  await thump.playOn(self, Math.min(1, speed / 6)).ended();
  level.set("thumps_heard", thumps);
}

export function onActivated(target: Entity, by: Player) {
  // A push is a mass times a speed: this sends it up at `hop` m/s whatever it weighs.
  self.push(vec3(0, hop * self.mass, 0));
}
