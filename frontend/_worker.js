function jsonError(message, status = 502) {
  return new Response(JSON.stringify({ error: message }), {
    status,
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "no-store",
    },
  });
}

function upstreamOrigin(env) {
  const scheme = env.API_SCHEME || "http";
  const origin = env.API_ORIGIN || "35.226.240.198:8082";
  if (!origin) {
    return "";
  }
  if (origin.startsWith("http://") || origin.startsWith("https://")) {
    return origin.replace(/\/$/, "");
  }
  return `${scheme}://${origin.replace(/\/$/, "")}`;
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (url.pathname.startsWith("/api/")) {
      const origin = upstreamOrigin(env);
      if (!origin) {
        return jsonError("API_ORIGIN is not configured in Cloudflare.");
      }
      const target = new URL(url.pathname.replace(/^\/api/, ""), origin);
      target.search = url.search;
      const proxyRequest = new Request(target.toString(), request);
      proxyRequest.headers.set("host", new URL(origin).host);
      return fetch(proxyRequest, {
        cf: {
          cacheTtl: 0,
          cacheEverything: false,
        },
      });
    }

    return env.ASSETS.fetch(request);
  },
};
