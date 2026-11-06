"use strict";

const form = document.getElementById("login-form");
const errorBox = document.getElementById("login-error");

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  errorBox.textContent = "";
  const button = form.querySelector("button");
  button.disabled = true;
  try {
    const response = await fetch("/api/login", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: form.email.value, password: form.password.value }),
    });
    if (response.ok) {
      window.location.replace("/");
      return;
    }
    const body = await response.json().catch(() => null);
    errorBox.textContent = body?.error?.message ?? "Не удалось войти, попробуйте ещё раз";
  } catch {
    errorBox.textContent = "Нет связи с сервером";
  } finally {
    button.disabled = false;
  }
});
