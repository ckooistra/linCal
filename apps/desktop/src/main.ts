import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import { createBackend } from "./services/backend";

const target = document.getElementById("app");
if (!target) throw new Error("Missing #app element");

// Settings (theme, hidden calendars) load before the first render.
const backend = await createBackend();
export default mount(App, { target, props: { backend } });
