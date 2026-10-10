import { make, type StableLeak } from "./lib/index.js"

export const value: StableLeak = { member: { value: "x" } }
export const made = make({ value: "y" })
