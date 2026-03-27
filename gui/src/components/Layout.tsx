import { useEffect, useState } from 'react';
import { NavLink, Outlet, useLocation } from 'react-router-dom';

const NAV_LINKS = [
  { to: '/', label: 'Dashboard' },
  { to: '/browse', label: 'Browse' },
  { to: '/settings', label: 'Settings' },
] as const;

function navLinkClass(mobile: boolean) {
  return ({ isActive }: { isActive: boolean }) => {
    const base = mobile
      ? 'block px-3 py-2 rounded text-sm font-medium transition-colors'
      : 'px-3 py-1.5 rounded text-sm font-medium transition-colors';
    return isActive
      ? `${base} bg-gray-900 text-white`
      : `${base} text-gray-600 hover:text-gray-900 hover:bg-gray-100`;
  };
}

function HamburgerIcon() {
  return (
    <svg className="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M4 6h16M4 12h16M4 18h16" />
    </svg>
  );
}

function CloseIcon() {
  return (
    <svg className="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
      <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
    </svg>
  );
}

export default function Layout() {
  const [menuOpen, setMenuOpen] = useState(false);
  const location = useLocation();

  // Close mobile menu on navigation (including browser back/forward)
  useEffect(() => { setMenuOpen(false); }, [location.pathname]);

  const handleNavClick = () => setMenuOpen(false);

  return (
    <div className="min-h-screen bg-gray-50">
      <header className="bg-white border-b border-gray-200">
        <div className="max-w-5xl mx-auto px-4 h-14 flex items-center justify-between">
          <div className="flex items-center gap-6">
            <span className="text-lg font-semibold text-gray-900 tracking-tight">
              Relava
            </span>
            {/* Desktop nav */}
            <nav className="hidden sm:flex gap-1">
              {NAV_LINKS.map(({ to, label }) => (
                <NavLink key={to} to={to} end={to === '/'} className={navLinkClass(false)}>
                  {label}
                </NavLink>
              ))}
            </nav>
          </div>

          {/* Mobile menu button */}
          <button
            type="button"
            onClick={() => setMenuOpen(!menuOpen)}
            className="sm:hidden p-1.5 rounded text-gray-600 hover:text-gray-900 hover:bg-gray-100 transition-colors"
            aria-label={menuOpen ? 'Close menu' : 'Open menu'}
            aria-expanded={menuOpen}
          >
            {menuOpen ? <CloseIcon /> : <HamburgerIcon />}
          </button>
        </div>

        {/* Mobile nav dropdown */}
        {menuOpen && (
          <nav className="sm:hidden border-t border-gray-100 px-4 py-2 space-y-1">
            {NAV_LINKS.map(({ to, label }) => (
              <NavLink
                key={to}
                to={to}
                end={to === '/'}
                className={navLinkClass(true)}
                onClick={handleNavClick}
              >
                {label}
              </NavLink>
            ))}
          </nav>
        )}
      </header>
      <main className="max-w-5xl mx-auto px-4 py-6 sm:py-8">
        <Outlet />
      </main>
    </div>
  );
}
