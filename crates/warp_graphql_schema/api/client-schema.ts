import fetch from 'cross-fetch';
import { getIntrospectionQuery, buildClientSchema, GraphQLSchema } from 'graphql';
import { filterSchema, pruneSchema } from '@graphql-tools/utils';

const clientMutations = [
  'addInviteLinkDomainRestriction',
  'addObjectGuests',
  'bulkCreateObjects',
  'createAgentTask',
  'createFolder',
  'createGenericStringObject',
  'createNotebook',
  'createTeam',
  'createWorkflow',
  'deleteConversation',
  'deleteInviteLinkDomainRestriction',
  'deleteObject',
  'deleteTeamInvite',
  'emptyTrash',
  'expireApiKey',
  'generateApiKey',
  'generateCodeEmbeddings',
  'generateCommands',
  'generateDialogue',
  'generateMetadataForCommand',
  'giveUpNotebookEditAccess',
  'grabNotebookEditAccess',
  'joinTeamWithTeamDiscovery',
  'leaveObject',
  'markAcceptedIntelligentAutosuggestion',
  'moveObject',
  'populateMerkleTreeCache',
  'recordObjectAction',
  'removeObjectGuest',
  'removeObjectLinkPermissions',
  'removeUserFromTeam',
  'renameTeam',
  'resetInviteLinks',
  'setObjectLinkPermissions',
  'sendTeamInviteEmail',
  'setIsInviteLinkEnabled',
  'setTeamDiscoverability',
  'setTeamMemberRole',
  'setUserIsOnboarded',
  'shareBlock',
  'transferGenericStringObjectOwner',
  'transferNotebookOwner',
  'transferTeamOwnership',
  'transferWorkflowOwner',
  'trashObject',
  'unshareBlock',
  'untrashObject',
  'updateFolder',
  'updateGenericStringObject',
  'updateNotebook',
  'updateMerkleTree',
  'updateObjectGuests',
  'updateWorkflow',
  'updateWorkspaceSettings',
];

const clientQueries = [
  'cloudObject',
  'codebaseContextConfig',
  'getRelevantFragments',
  'rerankFragments',
  'syncMerkleTree',
  'updatedCloudObjects',
  'user',
];

const clientSubscriptions = ['warpDriveUpdates'];

function filterToClient(schema: GraphQLSchema): GraphQLSchema {
  const filtered = filterSchema({
    schema,
    rootFieldFilter: (operation, rootFieldName) => {
      if (operation === 'Query') {
        return clientQueries.includes(rootFieldName);
      } else if (operation === 'Mutation') {
        return clientMutations.includes(rootFieldName);
      } else if (operation === 'Subscription') {
        return clientSubscriptions.includes(rootFieldName);
      } else {
        console.error(`Unknown operation ${operation}.${rootFieldName}`);
        return true;
      }
    }
  });

  return pruneSchema(filtered);
}

module.exports = async (schemaUrl: string) => {
  // Adapted from https://the-guild.dev/graphql/codegen/docs/config-reference/schema-field#custom-schema-loader
  const response = await fetch(schemaUrl, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ query: getIntrospectionQuery() }),
  });
  const data = await response.json();
  const schema = buildClientSchema(data.data);
  return filterToClient(schema);
};
