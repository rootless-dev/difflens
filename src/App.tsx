import { useState } from "react";

import { PROJECTS_ROUTE, type Route } from "@/app/route";
import { TopBar } from "@/app/TopBar";
import { FullAnalysesProvider } from "@/features/full-analysis/full-analyses";
import { RunningAnalyses } from "@/features/full-analysis/RunningAnalyses";
import { PreferencesScreen } from "@/features/preferences/PreferencesScreen";
import { ProjectScreen } from "@/features/project/ProjectScreen";
import { ProjectsScreen } from "@/features/projects/ProjectsScreen";
import { ReviewScreen } from "@/features/review/ReviewScreen";
import { targetKey } from "@/features/review/review-target";
import { TerminalFooter } from "@/features/terminal/TerminalFooter";
import { TerminalsProvider } from "@/features/terminal/terminals";
import { useTheme } from "@/hooks/use-theme";

function App() {
  const [route, setRoute] = useState<Route>(PROJECTS_ROUTE);
  const [barSlot, setBarSlot] = useState<HTMLDivElement | null>(null);
  const { theme, toggleTheme } = useTheme();

  return (
    // Full analyses run in the core while the app runs; every screen follows them from here. So do the terminal's
    // sessions, which outlive the screen they were opened on.
    <FullAnalysesProvider>
      <TerminalsProvider>
        <TopBar
          route={route}
          onNavigate={setRoute}
          theme={theme}
          onToggleTheme={toggleTheme}
          slotRef={setBarSlot}
          analyses={
            <RunningAnalyses
              onOpen={(job) =>
                setRoute({
                  screen: "review",
                  projectId: job.projectId,
                  projectName: job.projectName,
                  target: job.review.target,
                })
              }
            />
          }
        />
        {route.screen === "projects" && (
          <ProjectsScreen
            onOpenProject={(project) =>
              setRoute({ screen: "project", projectId: project.id, projectName: project.name })
            }
          />
        )}
        {route.screen === "project" && (
          <ProjectScreen
            key={route.projectId}
            projectId={route.projectId}
            projectName={route.projectName}
            onBack={() => setRoute(PROJECTS_ROUTE)}
            onReview={(target, opening, terminal) =>
              setRoute({
                screen: "review",
                projectId: route.projectId,
                projectName: route.projectName,
                target,
                opening,
                terminal,
              })
            }
            onOpenPreferences={() => setRoute({ screen: "preferences", from: route })}
          />
        )}
        {route.screen === "review" && (
          <ReviewScreen
            key={`${route.projectId}/${targetKey(route.target)}`}
            projectId={route.projectId}
            target={route.target}
            opening={route.opening}
            barSlot={barSlot}
            onOpenPreferences={() => setRoute({ screen: "preferences", from: route })}
          />
        )}
        {/* The terminal sits at the foot of the project screen and of a review, like a status bar. */}
        {(route.screen === "project" || route.screen === "review") && (
          <TerminalFooter
            key={`terminal/${route.projectId}/${route.screen === "review" ? targetKey(route.target) : "project"}`}
            projectId={route.projectId}
            target={route.screen === "review" ? route.target : null}
            request={route.screen === "review" ? route.terminal : undefined}
            theme={theme}
            onNavigate={setRoute}
          />
        )}
        {route.screen === "preferences" && <PreferencesScreen onBack={() => setRoute(route.from)} />}
      </TerminalsProvider>
    </FullAnalysesProvider>
  );
}

export default App;
