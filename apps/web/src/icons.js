import {
  createElement,
  Copy,
  Play,
  Pause,
  RefreshCw,
  GitBranch,
  KeyRound,
  ExternalLink,
  Cookie,
  Pencil,
  Settings2,
  Trash2,
  Check,
  ChevronRight,
  ChevronDown,
  Circle,
  Users,
  UserPlus,
  LayoutDashboard,
  CreditCard,
  Moon,
  Sun,
  PanelLeft,
  Search,
  Funnel,
  ArrowDownWideNarrow,
  Plus,
  X,
  Info,
  Upload,
  Download,
  Folder,
  ShieldCheck,
  Monitor,
  Server,
  LayoutGrid,
  List,
} from "lucide";

const shapes = {
  copy: Copy,
  play: Play,
  pause: Pause,
  refresh: RefreshCw,
  git: GitBranch,
  key: KeyRound,
  external: ExternalLink,
  cookie: Cookie,
  edit: Pencil,
  settings: Settings2,
  trash: Trash2,
  check: Check,
  "chevron-right": ChevronRight,
  "chevron-down": ChevronDown,
  circle: Circle,
  accounts: Users,
  registration: UserPlus,
  dashboard: LayoutDashboard,
  resources: CreditCard,
  moon: Moon,
  sun: Sun,
  sidebar: PanelLeft,
  search: Search,
  filter: Funnel,
  sort: ArrowDownWideNarrow,
  add: Plus,
  close: X,
  info: Info,
  upload: Upload,
  download: Download,
  folder: Folder,
  shield: ShieldCheck,
  monitor: Monitor,
  server: Server,
  grid: LayoutGrid,
  list: List,
};

export function iconSvg(name) {
  return createElement(shapes[name] || Circle, {
    width: 16,
    height: 16,
    "stroke-width": 1.7,
    "aria-hidden": "true",
    focusable: "false",
    "data-ui-icon": name,
  }).outerHTML;
}

export function setupIcons() {
  const projects = document.querySelector("[data-project-manager-icon]");
  if (projects) projects.outerHTML = iconSvg("folder");
  const addSymbol = document.querySelector(".account-add-trigger > [aria-hidden]");
  if (addSymbol) addSymbol.outerHTML = iconSvg("add");
  for (const [id, name] of [["card-export", "download"], ["card-delete", "trash"], ["runtime-diagnostics", "info"], ["task-browser-open", "monitor"], ["check-updates", "refresh"], ["install-update", "download"], ["download-update", "download"], ["copy-update-command", "copy"]]) {
    const button = document.getElementById(id);
    if (button) button.insertAdjacentHTML("afterbegin", iconSvg(name));
  }
  const buttons = {
    "#topbar-onboarding": "info",
    "#refresh-button": "refresh",
    '[data-page-target="dashboard"]': "dashboard",
    '[data-page-target="accounts"]': "accounts",
    '[data-page-target="registration"]': "registration",
    '[data-page-target="resources"]': "resources",
    '[data-page-target="settings"]': "settings",
    "#sidebar-toggle": "sidebar",
    '[data-account-bulk-action="clear-selection"]': "close",
    '[data-password-batch-open="passwords"]': "edit",
    '[data-password-batch-open="overleaf-password"]': "settings",
    '[data-account-bulk-action="refresh-session"]': "cookie",
    '[data-account-bulk-action="refresh-git"]': "git",
    '[data-account-bulk-action="generate-git"]': "key",
    '[data-account-bulk-action="browser-login"]': "external",
    '[data-compact-select-menu="account-filter"] summary': "filter",
    '[data-compact-select-menu="account-sort"] summary': "sort",
    ".account-status-menu summary": "list",
    "#account-toolbar-refresh": "refresh",
    "#account-toolbar-delete": "trash",
    'button[aria-label="导入账号"]': "upload",
    "#account-toolbar-export": "download",
    "#account-view-cards": "grid",
    "#account-view-table": "list",
    ".search-trigger-icon": "search",
  };
  for (const [selector, name] of Object.entries(buttons)) {
    document.querySelectorAll(selector).forEach((button) => {
      const old = button.querySelector("svg");
      if (old) old.outerHTML = iconSvg(name);
    });
  }
  document.querySelectorAll(".theme-icon").forEach((icon) => {
    const holder = document.createElement("span");
    holder.innerHTML = iconSvg(icon.classList.contains("theme-icon-sun") ? "sun" : "moon");
    const next = holder.firstElementChild;
    next.setAttribute("class", icon.getAttribute("class"));
    icon.replaceWith(next);
  });
  document.querySelectorAll(".settings-card-icon").forEach((el, i) => {
    el.innerHTML = iconSvg(["folder", "shield", "monitor", "server"][i] || "circle");
  });
  document.querySelectorAll(".delete-modal-icon").forEach((el) => {
    el.innerHTML = iconSvg("trash");
  });
}
