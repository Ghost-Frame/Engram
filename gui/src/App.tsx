import { BrowserRouter, Navigate, Route, Routes } from 'react-router';
import { RealtimeProvider } from '$lib/realtime';
import { AppShell } from './app/AppShell';
import { Spaces } from './routes/admin/Spaces';
import { Chiasm } from './routes/Chiasm';
import { Loom } from './routes/Loom';
import { Memory } from './routes/memory/Memory';
import { Overview } from './routes/Overview';
import { Soma } from './routes/Soma';
import { Stream } from './routes/Stream';
import { Thymus } from './routes/Thymus';

// Render the Kleos dashboard providers, router, and top-level routes.
export default function App() {
  return (
    <RealtimeProvider>
      <BrowserRouter>
        <Routes>
          <Route element={<AppShell />}>
            <Route index element={<Overview />} />
            <Route path="chiasm" element={<Chiasm />} />
            <Route path="broca" element={<Navigate replace to="/stream" />} />
            <Route path="stream" element={<Stream />} />
            <Route path="soma" element={<Soma />} />
            <Route path="loom" element={<Loom />} />
            <Route path="axon" element={<Navigate replace to="/stream" />} />
            <Route path="thymus" element={<Thymus />} />
            <Route path="memory/*" element={<Memory />} />
            <Route path="sharing" element={<Spaces />} />
            <Route path="*" element={<Navigate replace to="/" />} />
          </Route>
        </Routes>
      </BrowserRouter>
    </RealtimeProvider>
  );
}
