// Counts the times a player walks into a sector, in the level's $visits value (which
// materials can read as level:visits, and other scripts with level.get).

const door = input<Sector>();

export function onStart() {
  level.set("visits", 0);
}

export function onEntered(sector: Sector, by: Player) {
  const visits = level.get("visits", 0) + 1;
  level.set("visits", visits);
  if (visits > 1) console.log(door.name, "visits:", visits);
}
