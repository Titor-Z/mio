type Point = { x: number; y: number };

function greet(name: string): string {
  return `你好, ${name}`;
}

const p: Point = { x: 1, y: 2 };
console.log(greet("mio"), p.x + p.y);

enum Direction {
  Up = 1,
  Down = 2,
}

console.log("dir", Direction.Up + Direction.Down);
