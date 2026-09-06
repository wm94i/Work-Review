import test, { afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { get } from 'svelte/store';

import {
  applyUiTemplate,
  normalizeUiTemplate,
  uiTemplate,
} from './uiTemplate.ts';

const originalDocument = globalThis.document;

afterEach(() => {
  applyUiTemplate('classic');

  if (originalDocument === undefined) {
    Reflect.deleteProperty(globalThis, 'document');
  } else {
    Object.defineProperty(globalThis, 'document', {
      configurable: true,
      value: originalDocument,
    });
  }
});

test('界面模板应只接受 classic 和 evidence-star-map', () => {
  assert.equal(normalizeUiTemplate(' classic '), 'classic');
  assert.equal(normalizeUiTemplate('EVIDENCE-STAR-MAP'), 'evidence-star-map');
  assert.equal(normalizeUiTemplate('unknown'), 'classic');
  assert.equal(normalizeUiTemplate(null), 'classic');
});

test('界面模板 Store 默认应使用经典模式', () => {
  assert.equal(get(uiTemplate), 'classic');
});

test('应用界面模板时应同步 Store 和根节点 dataset', () => {
  const documentStub = {
    documentElement: {
      dataset: {} as DOMStringMap,
    },
  };
  Object.defineProperty(globalThis, 'document', {
    configurable: true,
    value: documentStub,
  });

  const applied = applyUiTemplate(' evidence-star-map ');

  assert.equal(applied, 'evidence-star-map');
  assert.equal(get(uiTemplate), 'evidence-star-map');
  assert.equal(
    documentStub.documentElement.dataset.uiTemplate,
    'evidence-star-map',
  );
});

test('应用非法模板时应回退并同步经典模式', () => {
  const documentStub = {
    documentElement: {
      dataset: { uiTemplate: 'evidence-star-map' } as DOMStringMap,
    },
  };
  Object.defineProperty(globalThis, 'document', {
    configurable: true,
    value: documentStub,
  });

  const applied = applyUiTemplate('invalid-template');

  assert.equal(applied, 'classic');
  assert.equal(get(uiTemplate), 'classic');
  assert.equal(documentStub.documentElement.dataset.uiTemplate, 'classic');
});
