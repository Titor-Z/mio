import { basename, dirname, extname, join, normalize, separator } from "@std/path";

const file = join("src", "engine", "boa_engine.ts");
console.log("file      :", file);
console.log("dirname   :", dirname(file));
console.log("basename  :", basename(file));
console.log("extname   :", extname(file));
console.log("normalize :", normalize("/a//b/../c"));
console.log("separator :", separator);
