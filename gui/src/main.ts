import { createApp } from "vue";
import App from "./App.vue";
import { initBackend } from "./backend";

initBackend().then(() => createApp(App).mount("#app"));
