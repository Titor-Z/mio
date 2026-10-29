export interface Greeter {
  name: string;
}

export function greet(g: Greeter): string {
  return `你好, ${g.name}`;
}
