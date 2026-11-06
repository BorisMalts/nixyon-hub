import { initBuilds } from "./builds.js";
import { $, api, initDialogs } from "./dom.js";
import { initQuality } from "./quality.js";
import { initStats } from "./stats.js";
import { initTabs } from "./tabs.js";
import { initTerminal } from "./terminal.js";
import { initTests } from "./tests.js";
import { initTraining } from "./training.js";

async function init() {
  try {
    const me = await api("/api/me");
    $("who").textContent = me.email;
  } catch {
    return;
  }
  $("logout").addEventListener("click", async () => {
    await api("/api/logout", { method: "POST" }).catch(() => null);
    window.location.replace("/login");
  });
  initDialogs();
  initBuilds();
  initTests();
  initQuality();
  initTraining();
  initTerminal();
  initStats();
  initTabs();
}

init();
