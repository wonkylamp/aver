import process from "node:process";
import { runCli } from "./cli.ts";

const result = runCli(process.argv.slice(2));
process.stdout.write(result.stdout);
process.stderr.write(result.stderr);
process.exitCode = result.code;
