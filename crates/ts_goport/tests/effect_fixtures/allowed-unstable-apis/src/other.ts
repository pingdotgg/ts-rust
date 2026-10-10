import { Command, Flag } from "effect/cli"
import { HttpClient } from "effect/http"

export const a = Command.make("a")
// Allowed by the root module subtree entry.
export const b = Flag.String("b")
// Allowed by the root exact entry.
export const c = HttpClient.HttpClient
export const d = HttpClient.get
