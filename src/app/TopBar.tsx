import type { ReactNode, Ref } from "react";
import { useTranslation } from "react-i18next";

import { PROJECTS_ROUTE, type Route } from "@/app/route";
import { Button } from "@/components/ui/button";
import { targetLabel } from "@/features/review/review-target";
import type { Theme } from "@/hooks/use-theme";

interface TopBarProps {
  route: Route;
  onNavigate: (route: Route) => void;
  theme: Theme;
  onToggleTheme: () => void;
  /** Element after the breadcrumb where a screen can portal its own bar content, such as the review chips. */
  slotRef?: Ref<HTMLDivElement>;
  /** The full analyses running, after the screen's slot: kept narrow, so the slot keeps its room. */
  analyses?: ReactNode;
}

export function TopBar({ route, onNavigate, theme, onToggleTheme, slotRef, analyses }: TopBarProps) {
  const { t } = useTranslation();
  return (
    <header className="flex h-[46px] flex-none items-center gap-3.5 border-b bg-card px-4">
      <div className="flex items-center gap-2 font-semibold tracking-[-0.01em]">
        <Logo />
        DiffLens
      </div>
      {/* Gives way before the review bar: the branch it truncates is also in the bar's compared chip. */}
      <nav
        aria-label={t("app.topBar.breadcrumb")}
        className="flex min-w-0 shrink-[4] items-center gap-2 text-muted-foreground"
      >
        <Button
          variant="ghost"
          size="xs"
          aria-current={route.screen === "projects" ? "page" : undefined}
          onClick={() => onNavigate(PROJECTS_ROUTE)}
        >
          {t("app.topBar.projects")}
        </Button>
        {(route.screen === "project" || route.screen === "review") && (
          <>
            <span aria-hidden="true">/</span>
            <Button
              variant="ghost"
              size="xs"
              aria-current={route.screen === "project" ? "page" : undefined}
              title={route.projectName}
              className="min-w-0 shrink truncate"
              onClick={() =>
                onNavigate({ screen: "project", projectId: route.projectId, projectName: route.projectName })
              }
            >
              <span className="truncate">{route.projectName}</span>
            </Button>
          </>
        )}
        {route.screen === "review" && (
          <>
            <span aria-hidden="true">/</span>
            <Button
              variant="ghost"
              size="xs"
              aria-current="page"
              className="min-w-0 shrink truncate"
              onClick={() => onNavigate(route)}
            >
              <span className="truncate">{targetLabel(route.target)}</span>
            </Button>
          </>
        )}
      </nav>
      {/*
        Doubles as the spacer that pushes the theme button to the right. A size container, so what a screen portals
        here can drop its less needed parts as the window narrows; the basis keeps room for the essential ones, since a
        size container is not sized by its content. It never gets narrower than that: what the review always shows
        there (pull request link, verdict buttons, Reload) measures 432px at most, in either language. When the
        running-analyses indicator takes room at the minimum window width, the breadcrumb gives way instead.
      */}
      <div ref={slotRef} className="@container flex min-w-[440px] flex-[1_1_440px] items-center gap-3.5" />
      {analyses}
      <Button
        variant="ghost"
        size="sm"
        aria-current={route.screen === "preferences" ? "page" : undefined}
        onClick={() => {
          if (route.screen !== "preferences") onNavigate({ screen: "preferences", from: route });
        }}
      >
        {t("app.topBar.preferences")}
      </Button>
      <Button variant="outline" size="sm" onClick={onToggleTheme}>
        {theme === "dark" ? t("app.topBar.lightTheme") : t("app.topBar.darkTheme")}
      </Button>
    </header>
  );
}

/**
 * A miniature of the app icon (design/app-icon.svg). Its colours are fixed rather than theme tokens, so it is
 * the same mark in both themes. The faint inner ring keeps the dark square apart from the dark theme's top bar.
 */
function Logo() {
  return (
    <svg width="20" height="20" viewBox="0 0 20 20" aria-hidden="true" className="block">
      <rect width="20" height="20" rx="4.5" fill="#052e35" />
      <rect
        x="0.5"
        y="0.5"
        width="19"
        height="19"
        rx="4"
        fill="none"
        stroke="#fce7cc"
        strokeOpacity="0.16"
      />
      <g fill="none" stroke="#fce7cc" strokeLinecap="round">
        <circle cx="9.5" cy="9.5" r="5.75" strokeWidth="1.5" />
        <path d="M14 14l2 2" strokeWidth="2" />
      </g>
      <rect x="6" y="7" width="7" height="2" rx="1" fill="#f65751" />
      <rect x="6" y="10" width="7" height="2" rx="1" fill="#54d8a3" />
    </svg>
  );
}
