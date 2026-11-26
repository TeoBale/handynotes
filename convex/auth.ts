import GitHub from "@auth/core/providers/github";
import type { TokenSet } from "@auth/core/types";
import { convexAuth } from "@convex-dev/auth/server";

const desktopRedirect =
  /^http:\/\/127\.0\.0\.1:([1-9][0-9]{0,4})\/oauth\/callback\/[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

export const { auth, signIn, signOut, store, isAuthenticated } = convexAuth({
  providers: [
    GitHub({
      authorization: { params: { scope: "read:user" } },
      checks: ["pkce", "state"],
      allowDangerousEmailAccountLinking: false,
      userinfo: {
        async request({ tokens }: { tokens: TokenSet }) {
          if (!tokens.access_token) {
            throw new Error("Missing GitHub access token");
          }
          const response = await fetch("https://api.github.com/user", {
            headers: {
              Authorization: `Bearer ${tokens.access_token}`,
              "User-Agent": "HandyNotes",
            },
          });
          if (!response.ok) {
            throw new Error("Could not load GitHub account");
          }
          return await response.json();
        },
      },
      profile({ id, login }) {
        if (!Number.isSafeInteger(id) || id <= 0 || typeof login !== "string") {
          throw new Error("Invalid GitHub account");
        }
        return { id: String(id), name: login };
      },
    }),
  ],
  callbacks: {
    async redirect({ redirectTo }) {
      const loopback = desktopRedirect.exec(redirectTo);
      if (loopback?.[0] === redirectTo && Number(loopback[1]) <= 65535) {
        return redirectTo;
      }

      const site = new URL(process.env.SITE_URL!);
      const destination = new URL(redirectTo, site);
      if (
        (destination.protocol === "https:" || destination.protocol === "http:") &&
        destination.origin === site.origin &&
        !destination.username &&
        !destination.password
      ) {
        return destination.href;
      }

      throw new Error("Invalid sign-in redirect");
    },
  },
});
