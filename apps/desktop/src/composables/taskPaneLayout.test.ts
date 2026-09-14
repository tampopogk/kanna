import { computed, ref } from 'vue';
import { describe, expect, it } from 'vitest';
import { useMainTabs } from './useMainTabs';
import { paneLeaves, paneRects, restorePaneLayout } from './taskPaneLayout';

describe('task pane ownership', () => {
  it('moves the agent between nested panes and restores the layout per task', () => {
    const scope = ref('item:one');
    const tabs = useMainTabs({ scopeKey: computed(() => scope.value) });
    tabs.openTab({ kind: 'file', filePath: 'readme.md' });
    tabs.splitPane('pane-1', 'horizontal');
    tabs.openTab({ kind: 'shell' });
    tabs.splitPane('pane-2', 'vertical');
    tabs.moveTab('agent', 'pane-3');
    expect(tabs.panes.value.map(rect => rect.pane.tabs)).toEqual([['file:readme.md'], ['shell', 'agent']]);
    expect(tabs.activeTabId.value).toBe('agent');
    expect(tabs.panes.value.map(rect => [rect.top, rect.height])).toEqual([[0, 50], [50, 50]]);
    const snapshot = tabs.snapshotScopes();
    scope.value = 'item:two';
    expect(tabs.panes.value[0].pane.tabs).toEqual(['agent']);
    scope.value = 'item:one';
    const restored = useMainTabs({ scopeKey: computed(() => scope.value) });
    restored.restoreScopes(snapshot);
    expect(restored.panes.value).toEqual(tabs.panes.value);
  });
  it('collapses a closed pane and keeps each view owned exactly once', () => {
    const tabs = useMainTabs({ scopeKey: computed(() => 'item:one') });
    tabs.openTab({ kind: 'diff' });
    tabs.splitPane('pane-1', 'horizontal');
    tabs.openTab({ kind: 'file', filePath: 'a.txt' });
    tabs.moveTab('file:a.txt', 'pane-1', 'agent');
    tabs.closeTab('diff');
    expect(tabs.panes.value).toHaveLength(1);
    expect(tabs.panes.value[0].pane.tabs).toEqual(['file:a.txt', 'agent']);
    expect(tabs.panes.value[0].width).toBe(100);
  });
  it('sanitizes stored duplicate tabs and missing views', () => {
    const layout = restorePaneLayout({kind:'split',axis:'horizontal',ratio:.5,
      first:{kind:'pane',id:'left',tabs:['agent','gone'],active:'gone'},
      second:{kind:'pane',id:'right',tabs:['agent','diff'],active:'diff'}}, ['agent','diff','shell'], 'agent');
    expect(paneLeaves(layout).flatMap(pane => pane.tabs)).toEqual(['agent','shell','diff']);
    expect(paneRects(layout).map(rect => rect.width)).toEqual([50,50]);
  });
});
