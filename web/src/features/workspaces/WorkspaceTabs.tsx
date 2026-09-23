import { NavLink } from "react-router";

export function WorkspaceTabs({ ws }: { ws: string }) {
  const cls = ({ isActive }: { isActive: boolean }) => (isActive ? "active" : "");
  return (
    <nav className="tabs">
      <NavLink end to={`/w/${ws}`} className={cls}>
        Sessions
      </NavLink>
      <NavLink to={`/w/${ws}/projects`} className={cls}>
        Projects
      </NavLink>
      <NavLink to={`/w/${ws}/settings`} className={cls}>
        Settings
      </NavLink>
      <NavLink to={`/w/${ws}/memory`} className={cls}>
        Memory
      </NavLink>
      <NavLink to={`/w/${ws}/cost`} className={cls}>
        Cost
      </NavLink>
    </nav>
  );
}
