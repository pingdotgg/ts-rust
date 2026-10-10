import { Command, Flag } from "effect/cli"
import { HttpClient } from "effect/http"

// Allowed by the override's exact entry.
export const a = Command.make("a")
// The override replaces the root list, so the Flag subtree is not allowed here.
export const b = Flag.String("b")
export const c = HttpClient.HttpClient
// Another export of an allowed module is not allowed.
export const d = HttpClient.get
