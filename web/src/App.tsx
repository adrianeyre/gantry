import { BrowserRouter, Route, Routes } from "react-router-dom";
import { AuthProvider, RequireAuth } from "./auth";
import { Layout } from "./components/Layout";
import { CamerasPage } from "./pages/CamerasPage";
import { FleetPage } from "./pages/FleetPage";
import { LoginPage } from "./pages/LoginPage";
import { JobsPage } from "./pages/JobsPage";
import { PrinterFormPage } from "./pages/PrinterFormPage";
import { PrintersPage } from "./pages/PrintersPage";
import { UsersPage } from "./pages/UsersPage";
import { AuditPage } from "./pages/AuditPage";
import { SettingsPage } from "./pages/SettingsPage";
import { AlertsPage } from "./pages/AlertsPage";
import { LibraryPage } from "./pages/LibraryPage";
import { ModelPage } from "./pages/ModelPage";
import { PrinterDetailPage } from "./pages/PrinterDetailPage";
import { QueuePage } from "./pages/QueuePage";
import { AnalyticsPage } from "./pages/AnalyticsPage";
import { FilamentPage } from "./pages/FilamentPage";
import { WallPage } from "./pages/WallPage";
import { ToolpathPage } from "./pages/ToolpathPage";
import { NotFoundPage } from "./pages/NotFoundPage";
import { AccountPage } from "./pages/AccountPage";
import { WebhooksPage } from "./pages/WebhooksPage";
import { RequireEnrolled } from "./pages/EnrolTotpPage";

export function AppRoutes() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage />} />
      <Route
        path="/wall"
        element={
          <RequireAuth>
            <RequireEnrolled>
              <WallPage />
            </RequireEnrolled>
          </RequireAuth>
        }
      />
      <Route
        element={
          <RequireAuth>
            <RequireEnrolled>
              <Layout />
            </RequireEnrolled>
          </RequireAuth>
        }
      >
        <Route index element={<FleetPage />} />
        <Route path="cameras" element={<CamerasPage />} />
        <Route path="printers" element={<PrintersPage />} />
        <Route path="printers/new" element={<PrinterFormPage />} />
        <Route path="printers/:id/edit" element={<PrinterFormPage />} />
        <Route path="printers/:id/jobs" element={<JobsPage />} />
        <Route path="printers/:id" element={<PrinterDetailPage />} />
        <Route path="library" element={<LibraryPage />} />
        <Route path="library/:id/model" element={<ModelPage />} />
        <Route path="library/:id/plates/:plate/toolpath" element={<ToolpathPage />} />
        <Route path="queue" element={<QueuePage />} />
        <Route path="filament" element={<FilamentPage />} />
        <Route path="analytics" element={<AnalyticsPage />} />
        <Route path="alerts" element={<AlertsPage />} />
        <Route path="webhooks" element={<WebhooksPage />} />
        <Route path="account" element={<AccountPage />} />
        <Route path="users" element={<UsersPage />} />
        <Route path="audit" element={<AuditPage />} />
        <Route path="settings" element={<SettingsPage />} />
        <Route path="*" element={<NotFoundPage />} />
      </Route>
    </Routes>
  );
}

export function App() {
  return (
    <BrowserRouter>
      <AuthProvider>
        <AppRoutes />
      </AuthProvider>
    </BrowserRouter>
  );
}
