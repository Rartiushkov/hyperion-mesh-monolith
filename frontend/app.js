const els = {
  log: document.getElementById("log"),
  authForm: document.getElementById("auth-form"),
  kycForm: document.getElementById("kyc-form"),
  sharedKycForm: document.getElementById("shared-kyc-form"),
  txCount: document.getElementById("tx-count"),
  kycLevel: document.getElementById("kyc-level"),
  gatewayStatus: document.getElementById("gateway-status"),
  widgetPlaceholder: document.getElementById("widget-placeholder"),
};

function writeLog(message, tone = "info") {
  const item = document.createElement("li");
  item.className = `log-item ${tone}`;
  item.textContent = `[${new Date().toLocaleTimeString()}] ${message}`;
  els.log.prepend(item);
}

function setGatewayStatus(text, tone = "ready") {
  els.gatewayStatus.textContent = text;
  els.gatewayStatus.dataset.tone = tone;
}

function updateState(data = {}) {
  if (typeof data.tx_count === "number") {
    els.txCount.textContent = String(data.tx_count);
  }
  if (typeof data.kyc_level === "string" && data.kyc_level) {
    els.kycLevel.textContent = data.kyc_level;
  }
}

function renderWidget(reason) {
  els.widgetPlaceholder.innerHTML = `
    <div class="widget-live">
      <strong>Open provider widget</strong>
      <p>${reason}</p>
      <button id="widget-complete" class="primary small">Widget finished</button>
    </div>
  `;

  const completeButton = document.getElementById("widget-complete");
  completeButton?.addEventListener("click", () => {
    writeLog("Provider widget completed on frontend side.", "success");
  });
}

function renderCodegoIframe(data) {
  els.widgetPlaceholder.innerHTML = `
    <div class="widget-live">
      <strong>Codego KYC session</strong>
      <p>Session ${data.session_id || "pending"} expires at ${data.expires_at || "unknown"}.</p>
      <iframe
        src="${data.iframe_url}"
        title="Codego KYC"
        style="width:100%;min-height:780px;border:0;border-radius:18px;background:#081815;"
        allow="camera"
      ></iframe>
    </div>
  `;
}

async function postJson(path, payload) {
  setGatewayStatus("sending", "busy");
  const response = await fetch(path, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "Accept": "application/json",
    },
    body: JSON.stringify(payload),
  });

  const text = await response.text();
  let data = {};
  try {
    data = text ? JSON.parse(text) : {};
  } catch {
    data = { raw: text };
  }

  if (!response.ok) {
    setGatewayStatus("error", "error");
    throw new Error(data.error || data.raw || `HTTP ${response.status}`);
  }

  setGatewayStatus("ready", "ready");
  return data;
}

els.authForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const payload = {
    user_id: document.getElementById("auth-user-id").value.trim(),
    account_id: document.getElementById("auth-account-id").value.trim(),
    card_id: document.getElementById("auth-card-id").value.trim(),
    merchant_id: document.getElementById("auth-merchant-id").value.trim(),
    currency: document.getElementById("auth-currency").value,
    amount_minor: Number(document.getElementById("auth-amount-minor").value),
  };

  try {
    const data = await postJson("/api/card/authorize", payload);
    updateState(data);
    writeLog(`Authorization verdict: ${data.verdict}`, data.verdict?.includes("Approved") ? "success" : "warn");
    if (String(data.frontend_command).includes("OpenCardProviderWidget")) {
      renderWidget("Transaction limit reached. Start a hosted Codego KYC session.");
      writeLog("Frontend received widget-open command.", "warn");
    }
  } catch (error) {
    writeLog(`Authorization failed: ${error.message}`, "error");
  }
});

els.kycForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const payload = {
    user_id: document.getElementById("kyc-user-id").value.trim(),
    account_id: document.getElementById("kyc-account-id").value.trim(),
    email: document.getElementById("kyc-email").value.trim(),
    origin: document.getElementById("kyc-origin").value.trim(),
    return_url: document.getElementById("kyc-return-url").value.trim(),
    locale: document.getElementById("kyc-locale").value.trim(),
    applicant_type: document.getElementById("kyc-applicant-type").value,
  };

  try {
    const data = await postJson("/api/codego/kyc/session", payload);
    updateState(data);
    writeLog(`Codego session accepted=${data.accepted} provider_status=${data.provider_http_status}`, data.accepted ? "success" : "warn");
    if (data.iframe_url) {
      renderCodegoIframe(data);
      writeLog("Hosted Codego iframe opened in the frontend.", "success");
    }
  } catch (error) {
    writeLog(`Codego KYC session failed: ${error.message}`, "error");
  }
});

els.sharedKycForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const rawPassportPayload = document.getElementById("shared-passport-json").value.trim();
  let passportPayloadJson = rawPassportPayload;

  try {
    passportPayloadJson = JSON.stringify(JSON.parse(rawPassportPayload));
  } catch (error) {
    writeLog(`Passport payload JSON is invalid: ${error.message}`, "error");
    return;
  }

  const payload = {
    user_id: document.getElementById("shared-user-id").value.trim(),
    account_id: document.getElementById("shared-account-id").value.trim(),
    email: document.getElementById("shared-email").value.trim(),
    origin: document.getElementById("shared-origin").value.trim(),
    return_url: document.getElementById("shared-return-url").value.trim(),
    passport_payload_json: passportPayloadJson,
  };

  try {
    const data = await postJson("/api/shared-kyc/process", payload);
    updateState(data);
    writeLog(
      `Shared KYC transit accepted=${data.accepted} provider_status=${data.provider_http_status}`,
      data.accepted ? "success" : "warn",
    );
    if (data.iframe_url) {
      renderCodegoIframe(data);
      writeLog("Transit flow returned a hosted continuation iframe.", "success");
    }
  } catch (error) {
    writeLog(`Shared KYC transit failed: ${error.message}`, "error");
  }
});

if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/service-worker.js").catch(() => {
    writeLog("Service worker registration failed.", "warn");
  });
}

window.addEventListener("message", (event) => {
  if (event.origin !== "https://kyc-sandbox.codegotech.com") {
    return;
  }
  if (event.data?.type === "kyc:done") {
    writeLog("Codego iframe reported kyc:done. Waiting for the user.updated webhook.", "info");
  }
});

writeLog("Frontend ready. Cloudflare should proxy /api/* to the GCP webhook.", "info");
