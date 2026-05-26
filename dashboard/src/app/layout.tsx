import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";
import Script from "next/script";
import "./globals.css";
import { Sidebar } from "@/components/sidebar";
import { AuthGate } from "@/components/auth-gate";
import { LibraryProvider } from "@/contexts/library-context";
import { MissionSwitcherProvider } from "@/contexts/mission-switcher-context";
import { ToastProvider } from "@/components/toast";
import { DevFetchThrottleInstaller } from "@/components/dev-fetch-throttle-installer";
import { BackendPreconnect } from "@/components/backend-preconnect";

const geistSans = Geist({
  variable: "--font-geist-sans",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

export const metadata: Metadata = {
  title: "Sandboxed.sh",
  description: "Autonomous coding agents in isolated environments",
  icons: {
    icon: "/favicon.svg",
  },
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body
        className={`${geistSans.variable} ${geistMono.variable} antialiased`}
      >
        <Script id="theme-init" strategy="beforeInteractive">
          {`
              (() => {
                try {
                  const stored = localStorage.getItem("sandboxed-theme");
                  const system = window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
                  const theme = stored === "light" || stored === "dark" ? stored : system;
                  document.documentElement.dataset.theme = theme;
                } catch {
                  document.documentElement.dataset.theme = "dark";
                }
              })();
            `}
        </Script>
        <BackendPreconnect />
        <DevFetchThrottleInstaller />
        <AuthGate>
          <ToastProvider>
            <LibraryProvider>
              <MissionSwitcherProvider>
                <Sidebar />
                <main className="ml-56 min-h-screen">{children}</main>
              </MissionSwitcherProvider>
            </LibraryProvider>
          </ToastProvider>
        </AuthGate>
      </body>
    </html>
  );
}
