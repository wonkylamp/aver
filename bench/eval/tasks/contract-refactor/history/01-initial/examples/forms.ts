// Checking a signup form the way the web app does.
import { collapseWhitespace, isEmail, isPostcodeUK, truncate } from "../src/index.ts";

const form = { name: "  Ada   Lovelace ", email: "ada@example.com", postcode: "sw1a1aa", bio: "Mathematician and writer, chiefly known for her work on the Analytical Engine." };

const errors: string[] = [];
if (!isEmail(form.email)) errors.push("email");
if (!isPostcodeUK(form.postcode)) errors.push("postcode");
console.log({ name: collapseWhitespace(form.name), bio: truncate(form.bio, 40), errors });
