import BookOpen from 'lucide-svelte/icons/book-open';
import BriefcaseBusiness from 'lucide-svelte/icons/briefcase-business';
import Camera from 'lucide-svelte/icons/camera';
import ChartNoAxesColumn from 'lucide-svelte/icons/chart-no-axes-column';
import Code2 from 'lucide-svelte/icons/code-2';
import FileText from 'lucide-svelte/icons/file-text';
import FlaskConical from 'lucide-svelte/icons/flask-conical';
import Folder from 'lucide-svelte/icons/folder';
import Gamepad2 from 'lucide-svelte/icons/gamepad-2';
import Globe2 from 'lucide-svelte/icons/globe-2';
import House from 'lucide-svelte/icons/house';
import Lightbulb from 'lucide-svelte/icons/lightbulb';
import LockKeyhole from 'lucide-svelte/icons/lock-keyhole';
import MessageCircle from 'lucide-svelte/icons/message-circle';
import Music2 from 'lucide-svelte/icons/music-2';
import Palette from 'lucide-svelte/icons/palette';
import Rocket from 'lucide-svelte/icons/rocket';
import Smartphone from 'lucide-svelte/icons/smartphone';
import Star from 'lucide-svelte/icons/star';
import Tag from 'lucide-svelte/icons/tag';
import Target from 'lucide-svelte/icons/target';
import Wrench from 'lucide-svelte/icons/wrench';
import Zap from 'lucide-svelte/icons/zap';

export const categoryIconOptions = [
  { key: 'code', component: Code2 },
  { key: 'web', component: Globe2 },
  { key: 'message', component: MessageCircle },
  { key: 'document', component: FileText },
  { key: 'design', component: Palette },
  { key: 'game', component: Gamepad2 },
  { key: 'folder', component: Folder },
  { key: 'energy', component: Zap },
  { key: 'chart', component: ChartNoAxesColumn },
  { key: 'tool', component: Wrench },
  { key: 'idea', component: Lightbulb },
  { key: 'target', component: Target },
  { key: 'tag', component: Tag },
  { key: 'home', component: House },
  { key: 'book', component: BookOpen },
  { key: 'music', component: Music2 },
  { key: 'camera', component: Camera },
  { key: 'lab', component: FlaskConical },
  { key: 'work', component: BriefcaseBusiness },
  { key: 'phone', component: Smartphone },
  { key: 'rocket', component: Rocket },
  { key: 'star', component: Star },
  { key: 'lock', component: LockKeyhole },
] as const;

export type CategoryIconKey = (typeof categoryIconOptions)[number]['key'];

const iconComponents = Object.fromEntries(
  categoryIconOptions.map((option) => [option.key, option.component]),
) as Record<CategoryIconKey, typeof Tag>;

const systemCategoryIcons: Record<string, CategoryIconKey> = {
  development: 'code',
  browser: 'web',
  communication: 'message',
  office: 'document',
  design: 'design',
  entertainment: 'game',
  other: 'folder',
};

const categoryIconKeys = new Set<string>(categoryIconOptions.map((option) => option.key));

export function normalizeCategoryIconKey(iconKey: string | null | undefined): CategoryIconKey {
  return iconKey && categoryIconKeys.has(iconKey) ? iconKey as CategoryIconKey : 'tag';
}

export function resolveCategoryIcon(categoryKey: string, iconKey?: string | null) {
  const resolvedKey = systemCategoryIcons[categoryKey] ?? normalizeCategoryIconKey(iconKey);
  return iconComponents[resolvedKey];
}
