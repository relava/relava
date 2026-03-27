import { NavLink, Outlet } from 'react-router-dom';

const NAV_LINKS = [
  { to: '/', label: 'Dashboard' },
  { to: '/browse', label: 'Browse' },
  { to: '/settings', label: 'Settings' },
] as const;

function navClass({ isActive }: { isActive: boolean }) {
  const base = 'px-3 py-1 rounded text-sm font-medium transition-colors';
  return isActive
    ? `${base} bg-gray-900 text-white`
    : `${base} text-gray-600 hover:text-gray-900 hover:bg-gray-100`;
}

export default function Layout() {
  return (
    <div className="min-h-screen bg-gray-50">
      <header className="bg-white border-b border-gray-200">
        <div className="max-w-5xl mx-auto px-4 h-14 flex items-center gap-6">
          <span className="text-lg font-semibold text-gray-900 tracking-tight">
            Relava
          </span>
          <nav className="flex gap-1">
            {NAV_LINKS.map(({ to, label }) => (
              <NavLink key={to} to={to} end={to === '/'} className={navClass}>
                {label}
              </NavLink>
            ))}
          </nav>
        </div>
      </header>
      <main className="max-w-5xl mx-auto px-4 py-8">
        <Outlet />
      </main>
    </div>
  );
}
