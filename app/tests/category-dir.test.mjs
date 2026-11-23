// Regression test for #26: with "sort by file type" on, the category folder
// was built with a hardcoded backslash, so on Linux downloads landed in a
// folder literally named "Downloads\Video" instead of "Downloads/Video".
import assert from "node:assert/strict";
import test from "node:test";
import { categoryDir } from "../ui/src/util.ts";

test("uses a forward slash for POSIX download directories", () => {
  assert.equal(categoryDir("/home/me/Downloads", "Video", true), "/home/me/Downloads/Video");
});

test("keeps backslashes for Windows download directories", () => {
  assert.equal(categoryDir("C:\\Users\\me\\Downloads", "Video", true), "C:\\Users\\me\\Downloads\\Video");
});

test("leaves the base directory untouched for Other or when sorting is off", () => {
  assert.equal(categoryDir("/home/me/Downloads", "Other", true), "/home/me/Downloads");
  assert.equal(categoryDir("/home/me/Downloads", "Video", false), "/home/me/Downloads");
});

test("does not duplicate a separator already present in the base directory", () => {
  assert.equal(categoryDir("/home/me/Downloads/", "Music", true), "/home/me/Downloads/Music");
  assert.equal(categoryDir("C:\\Users\\me\\Downloads\\", "Music", true), "C:\\Users\\me\\Downloads\\Music");
});
