import { greet } from "./greet.ts";
import { add } from "./math/index.ts";

const message: string = greet({ name: "mio" });
console.log(message);
console.log("sum", add(2, 3));
